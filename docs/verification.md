# Verification

This document lists how Strata is checked, what each check covers, and where
its results are recorded. Every result file in `results/` states its
category; everything in this document is either a simulation result or a
formal proof by a tool. Implementation results for the FPGA are in
[fpga.md](fpga.md) and for the layout in [physical.md](physical.md).

| Level | Check | Tool | Scope | Command | Results |
|-------|-------|------|-------|---------|---------|
| Instructions | riscv-tests conformance suite | strata-run (RTL, gate, lockstep) | rv32ui, rv32mi | `scripts/isa_tests.py --suite riscv-tests` | `results/isa/` |
| Instructions | directed tests of the privileged architecture | strata-run | CSRs, counters, traps, PMP, interrupts | `scripts/isa_tests.py --suite strata` | `results/isa/` |
| Instructions | riscv-formal bounded model checks | SymbiYosys, yices | every RV32I instruction, register file, PC, liveness | `scripts/formal.py` | `results/formal/` |
| RTL vs netlist | formal equivalence, per module | yosys `equiv_*` | all 21 module instances of `strata_soc` | `scripts/equiv.py` | `results/equiv/` |
| RTL vs netlist | lockstep simulation, every output every cycle | strata-run `--engine lockstep` | ISA tests and whole-machine sessions | `isa_tests.py --engine lockstep`, `machine_tests.py` | `results/isa/`, `results/machine/` |
| Netlist vs layout | structural comparison, then formal equivalence of the routed netlist | netlist_compare.py, kepler-formal | the whole block | `scripts/physical.py` | `results/physical/` |
| Cells | logic function of every cell type at transistor level | ngspice | 76 cell types | `strata-spice verify` | `results/transistor/` |
| Four-state | X checking with 4-state simulation | Icarus Verilog | ISA tests, boot to the shell | `scripts/four_state.py` | `results/four_state/` |
| Whole machine | scripted sessions with expected transcripts and screens | strata-run (gate, lockstep, RTL) | boot, shell, multitasking, faults, game, boot failures | `scripts/machine_tests.py` | `results/machine/` |
| Board | FPGA top with DVI receiver | Verilator | PLL, reset, flash, TMDS path | `scripts/fpga_sim.py` | `results/fpga/board_sim.json` |
| Debugger | session semantics (replay, checkpoints, triggers, faults) | cargo test | `strata-debug` | `cargo test -p strata-debug` | test output |

## Instruction behavior

### riscv-tests

The upstream [riscv-tests](https://github.com/riscv-software-src/riscv-tests)
`rv32ui` (user-level integer) and `rv32mi` (machine-mode) tests run on the
machine unchanged except for their environment. `verif/riscv-tests/env/`
replaces the test environment header: tests are linked for RAM at
`0x10000000` and report their result by writing the halt register
(`SYS.HALT`), and `verif/riscv-tests/rom_stub.S` is a two-instruction boot ROM
that jumps to RAM. The test image is preloaded into RAM (a test-harness
shortcut; see [abstractions.md](abstractions.md)).

Four upstream tests are skipped because they need features that S1 does not
have, each for a stated reason:

| Test | Reason |
|------|--------|
| `rv32mi-breakpoint` | debug triggers (`tselect`/`tdata`) are not implemented |
| `rv32mi-illegal` | exercises S-mode (`sret`, `sfence.vma`, `satp`) |
| `rv32mi-ma_fetch` | requires compressed instructions |
| `rv32ui-ma_data` | requires misaligned loads and stores, which trap on S1 |

All 54 others pass on the RTL model, on the gate-level model, and in
lockstep, where the RTL and gate models are compared on every output in
every cycle.

### Directed tests

`verif/isa/*.S` test the privileged architecture as specified in
[isa.md](isa.md), including behavior that riscv-tests does not reach:

| Test | Covers |
|------|--------|
| `csr.S` | identification registers, reset values, WARL fields, read-only values |
| `counters.S` | `mcycle`, `minstret` and `time` counting and their high halves, `time` matching the TIMER, `mcounteren` gating of U-mode reads |
| `faults.S` | access faults from the memory map, misalignment, illegal instructions and CSR accesses, breakpoints, environment calls, and precision (nothing after a faulting instruction takes effect), with `mepc` and `mtval` for each |
| `pmp.S` | TOR, NA4 and NAPOT matching, priority between entries, U-mode fetch, load and store checks, the default for unmatched U-mode accesses, `MPRV`, WARL behavior, locking |
| `irq.S` | timer, software and external interrupts, priority between them, vectored `mtvec`, `WFI` with interrupts disabled, interrupts taken from U-mode |

The test framework (`verif/isa/strata_test.h`) records every trap in a
history buffer so that tests can check the order of traps, not just the last
one. A deliberately wrong expectation was used to confirm that a failing
check is reported.

### Formal checks

`scripts/formal.py` sets up [riscv-formal](https://github.com/YosysHQ/riscv-formal)
for S1 (`verif/formal/`: a wrapper that connects the core to unconstrained
bus responses and exposes the RVFI trace port). The configuration
(`verif/formal/checks.cfg`) runs 44 checks at depth 12 with yices: one per
RV32I instruction (37), register file consistency (`reg`), PC continuity
forward and backward (`pc_fwd`, `pc_bwd`), unique retirement order
(`unique`), causality (`causal`), illegal instruction handling (`ill`), and a
cover check that the core can retire instructions. All 44 pass.

The wrapper (`verif/formal/wrapper.sv`) answers both buses with arbitrary data
after arbitrary delays, as the bus protocol allows ([bus.md](bus.md)), and
asserts that the core follows the protocol. Interrupt lines and `mtime` are
unconstrained. The checks are made under these constraints:

* no bus error responses (the `STRATA_FV_BUS_ERRORS` define lifts this);
* instructions execute in M-mode, loads and stores use M-mode permissions,
  and no PMP entry is locked, because riscv-formal's instruction models do
  not include privilege levels or PMP.

The CSR and trap checks of riscv-formal are not configured. Privilege, PMP,
traps and bus errors are covered by the directed tests instead.

## RTL versus netlist

### Formal equivalence

`scripts/equiv.py` proves each of the 21 module instances of the gate-level
netlist equivalent to its RTL. For each module the gold design is the RTL
elaborated by yosys-slang and optimized (`opt -full`, which removes register
bits whose value can never be observed); the gate design is the module from
`build/gate/strata_soc.v` with each cell's function taken from its Liberty
definition. Child instances are cut at their ports on both sides, so every
module is proven against its own RTL. yosys proves the matched signal pairs
with `equiv_induct` (temporal induction, 4 steps) followed by `equiv_simple`.
All 21 modules pass, 10,211 equivalence points proven, none unproven.

### Lockstep simulation

`strata-run --engine lockstep` runs the Verilator model of the RTL and the
gate-level model side by side from the same inputs and compares every output
of `strata_soc` (pins, memory ports, the trace port) in every cycle; the first
difference stops the run with the signal and cycle. The ISA suites and the
`shell_tour` whole-machine session (10.75 million cycles: boot, shell,
programs, faults and the game) run in lockstep with no difference.

### Netlist versus layout

The layout flow changes the netlist (clock tree, buffers, resizing, tie
cells). `scripts/physical.py` checks that the netlist OpenROAD read is
structurally the simulated one, and proves the final routed netlist
equivalent to it with kepler-formal. See
[physical.md](physical.md#correspondence-with-the-simulated-netlist).

### Power-on state

The gate model can start with pseudo-random flip-flop contents
(`--power-on <seed>`). The `power_on_*` whole-machine tests boot with
different seeds and must produce the same transcript and screen as a boot
from all-zero state, which shows that reset initializes all the state that
matters.

## Four-state simulation

The gate model and Verilator are two-state. `scripts/four_state.py` runs the
RTL in Icarus Verilog, where uninitialized flip-flops and memory are X, with
`verif/tb/tb_machine.sv`, which stops with an error if any X appears on the
control outputs or the bus handshakes after reset. It runs every ISA test and
a boot from the flash to the shell prompt.

## Whole-machine tests

`scripts/machine_tests.py` runs complete sessions on the machine: boot ROM,
flash, kernel and user programs, driven only through the pins (terminal,
buttons) by scripts in `verif/machine/`. Each test checks the halt code, the
complete terminal transcript against `verif/machine/expected/<test>.txt`, and
each screenshot (the frame captured from the video pins) against an expected
PNG. The expected files are regenerated with `--update` and reviewed like
source.

| Test | Model | What it shows |
|------|-------|---------------|
| `boot` | gate | boot from the flash to the shell prompt, screen |
| `power_on_1`, `power_on_2` | gate | same boot from pseudo-random power-on state |
| `shell_tour` | lockstep | shell commands, programs, fault containment, game benchmark |
| `multitask` | gate | a background task counting on the LEDs while the shell runs |
| `crash` | gate | all six kinds of user fault reported and contained |
| `game` | gate | the game played with the board buttons, screens while playing and paused |
| `blocks_bench` | RTL | CPU-bound run of the computer player, 40 pieces |
| `uart_boot` | RTL | no kernel in the flash: kernel loaded over the serial line |
| `bad_crc` | RTL | corrupted kernel in the flash rejected, recovery over the serial line |
| `no_store` | RTL | erased flash: kernel panic, halt code `0xEE` |

The RTL model is used where a test is long and the gate model adds nothing
that the lockstep runs do not already show. `--quick` runs every test on the
RTL model; its transcripts and screens must equal the expected ones produced
by the gate model.

Writing these tests found two software defects: the boot ROM printed a
message while an image was arriving on the serial line, so the 8-byte
receive FIFO overflowed and the serial boot path failed; and the `crash 6`
case (execute from the stack) was compiled into read-only data, which is
executable, so it did not test what it claimed. Both are fixed.

## Board-level simulation

See [fpga.md](fpga.md#simulated-results): the FPGA top runs under Verilator
with models of the ECP5 PLL and DDR output cells, boots from the flash model,
and a DVI receiver in the testbench checks every TMDS symbol against the
machine's video outputs; the decoded frame must equal the frame from the RTL
and gate models.

## Running everything

```
scripts/fetch_third_party.sh
make -C sw && python3 scripts/synth_gate.py
python3 scripts/isa_tests.py --engine lockstep
python3 scripts/formal.py
python3 scripts/equiv.py
python3 scripts/four_state.py
python3 scripts/machine_tests.py
python3 scripts/fpga_sim.py
python3 scripts/physical.py
tools/target/release/strata-spice verify
cargo test --release --manifest-path tools/Cargo.toml
```
