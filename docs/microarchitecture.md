# S1 microarchitecture

S1 is Strata's CPU: an original, in-order, single-issue, five-stage pipeline
implementing the architecture in [isa.md](isa.md). The RTL is in `rtl/core/`.

```
            +-----------+     +-----------+     +-----------+     +-----------+     +-----------+
 ibus <---->|  F fetch  |---->| D decode  |---->| X execute |---->| M memory  |---->| W commit  |
            | st_fetch  |     | st_decode |     | st_alu    |     | dbus cmd  |     | st_csr    |
            | skid buf  |     | regfile rd|     | branch    |     | interrupt |     | regfile wr|
            +-----------+     +-----------+     | PMP data  |     +-----------+     +-----------+
                 ^  ^                           +-----------+          |                 |
                 |  +----- redirect (taken branch/jump) -----+          |                 |
                 +------------- redirect (trap, MRET, serialize) ------------------------+
                                                     ^ forwarding from M and W             |
 dbus <------------------------------------------------------------ cmd (M), rsp (W) ---+
```

| Module | Role | sky130 cells |
|--------|------|-------------:|
| `st_core`    | pipeline registers, forwarding, hazards, W-stage control | 2,729 (own) |
| `st_fetch`   | fetch unit | 355 |
| `st_decode`  | decoder | 151 |
| `st_regfile` | 31 x 32-bit registers, 2 read ports | 2,636 |
| `st_alu`     | ALU | 1,020 |
| `st_csr`     | CSRs, privilege, traps, counters, interrupts | 2,283 |
| `st_pmp` x 2 | PMP checkers (fetch, data) | 638 each |

(Cell counts from `build/gate/gate_stat.txt`, typical corner mapping.)

## Stages

**F (fetch).** `st_fetch` holds the next sequential fetch address and issues
one instruction-bus read per cycle while decode keeps up. At most one fetch is
outstanding. A returning instruction goes straight to decode, or into a
one-entry skid buffer if decode is stalled; a new sequential fetch is issued
only if its response will find the skid buffer empty. A redirect issues the
fetch of the new target in the same cycle (the previous fetch is always
completing in that cycle, since every instruction-bus slave responds in one
cycle), and marks any older fetch still in flight to be discarded.

Instruction-bus reads have no side effects, so the fetch-side PMP check is
applied to the *returning* fetch (its registered address), not to the issued
address. A PMP failure or an error response turns the instruction into an
instruction-access-fault carrier.

**D (decode).** Combinational: `st_decode` produces a control bundle
(`ctrl_t`) and the immediate, and the register file is read. The value being
written by W in the same cycle is bypassed into the read. Illegal encodings
become exception carriers with all side-effect flags cleared.

**X (execute).** Operands are taken from M (youngest), then W, then the
values read in D. The ALU, the branch comparator, and the branch/jump target
adder operate here. A taken branch or jump redirects fetch in the cycle it
leaves X (one bubble). Load/store addresses are computed, checked for
alignment and against PMP (with the effective privilege, honoring `MPRV`), and
store data is placed in its byte lanes. X raises instruction-address-
misaligned (on the jump), load/store-misaligned and PMP access faults.

**M (memory).** The data-bus command is issued here, only when the
instruction will enter W in the same cycle and W's own access (if any) has
completed without error. Interrupts are taken on the instruction leaving M:
if an enabled interrupt is pending, that instruction is not executed (no bus
access) and becomes an interrupt carrier.

**W (commit).** Load data arrives and is aligned and extended. W is the only
place where architectural state changes other than memory: register write,
CSR access (read and write), counters, trap entry, MRET, privilege change.
Exceptions from all stages are taken here, in program order, with precise
`mepc`. After a CSR write, `FENCE.I`, MRET or a trap, W flushes every younger
instruction and redirects fetch, so no instruction ever runs with stale CSR,
PMP or privilege state.

## Stalls and flushes

Each stage advances when the next stage can accept it:

```
w_ready_in = !w_valid || w_done          w_done: W completes (retires or traps)
m_go       = m_valid && w_ready_in && !halt && (no bus command needed || cmd_ready)
x_go       = x_valid && m_ready_in && !x_hazard
d_go       = fetch has an instruction && x_ready_in
```

W does not complete while it waits for a data-bus response, while a `WFI`
waits for `(mip & mie) != 0`, or after `SYS.HALT` (except to finish the
halting store itself).

`x_hazard` stalls X when a source register is produced by

* the instruction in M and that instruction is a load or a CSR instruction
  (its value only exists in W), or
* the instruction in W and that instruction is a load still waiting for its
  response.

A load followed immediately by a dependent instruction therefore costs one
cycle. While X is held, its operand registers are refreshed from the
forwarding network every cycle, so a producer that retires meanwhile is
never missed.

A W flush (trap, MRET, serialization) clears M, X, D and fetch. An X redirect
clears D and fetch; the branch itself continues.

## Traps, interrupts and WFI

| Event | Detected in | Taken in |
|-------|-------------|----------|
| instruction access fault | F (response) | W |
| illegal instruction (encoding) | D | W |
| illegal instruction (CSR access, MRET in U, WFI with TW) | W | W |
| instruction address misaligned | X | W |
| load/store misaligned, PMP fault | X | W |
| load/store access fault (bus error) | W (response) | W |
| ECALL, EBREAK | D | W |
| interrupt | M | W |

Because the trap point is W and data accesses are issued from M only when W
is known not to trap, every exception is precise and a faulting access has no
side effects. An interrupt is attached to the instruction leaving M; it
reaches W one cycle later and traps with `mepc` pointing at it. A `WFI` in W
waits with the next instruction held in M; when an interrupt becomes pending
the `WFI` completes and, if interrupts are enabled, that next instruction
carries the interrupt.

## Performance

In the machine every slave responds in one cycle, so:

| Situation | Cost |
|-----------|------|
| any instruction, no hazards | 1 cycle |
| taken branch, `JAL`, `JALR` | +1 cycle |
| load (or CSR read) followed by a dependent instruction | +1 cycle |
| decode stall (skid refill after a load-use or other stall) | up to +1 cycle |
| CSR write, `FENCE.I`, `MRET` | +3 cycles (refill) |
| trap entry | +3 cycles |

The smoke test (`verif/smoke`) retires 2,792 instructions in 7,272 cycles
after reset, dominated by polling the UART.

## Observation: the retirement trace

W drives a set of `trace_*` outputs through `strata_soc`: for every
instruction that leaves W, its PC, encoding, next PC, privilege mode,
register write, and whether it trapped (with cause). The debugger, the
simulators and the tests read these pins; they are part of the gate-level
netlist and are evaluated like any other logic. With `RISCV_FORMAL` defined
the core also provides the standard RVFI port for formal verification.
