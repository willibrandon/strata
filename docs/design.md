# Strata design

Strata is an original computer built from the gate level up, together with a
debugger that shows how its software runs on its hardware. This document is
the top-level design: the requirements, the architecture, the major
decisions, and where each part is specified and implemented.

## 1. Requirements

The project brief, verbatim:

> Build an original, synthesizable computer and a native debugging environment that connects its software execution to its hardware implementation.
>
> Design the processor, memory system, interconnect, and peripherals in synthesizable SystemVerilog. Implement an original RV32I microarchitecture with the explicitly documented privileged facilities needed by a small operating system. Do not reuse an existing CPU core.
>
> Produce an executable gate-level model from the hardware design. In gate mode, the circuit must determine execution; no instruction interpreter or host-side game logic may substitute for it. Clearly identify every memory or device abstraction.
>
> Build the assembler, boot software, drivers, small operating system, and a game. Guest software must interact only with the machine's documented interfaces.
>
> Develop a debugger that connects source and assembly to instruction execution, memory transactions, hardware modules, and actual netlist cells. Support deterministic replay, checkpoints, waveform inspection, precise triggers, and controlled fault injection. Build 3D navigation around this inspection model, not around decorative hardware.
>
> Verify instruction behavior, check RTL-to-netlist equivalence where supported, and provide reproducible whole-machine tests. Deliver an FPGA implementation flow and distinguish simulated results, implementation-tool results, and physically tested results.
>
> Extend the system with transistor-level analysis of selected cells and a physical-layout view derived from real design-tool output. State the scope and fidelity of each mode.
>
> Success means the computer works without the visualizer, the visualizer accurately explains the computer, and the design has a demonstrated path to physical hardware.

### 1.1 Requirement map

| Requirement | Design decision | Specification | Implementation |
|-------------|-----------------|---------------|----------------|
| Original synthesizable computer | Single-clock SoC in SystemVerilog, written for this project | [memory-map.md](memory-map.md), [bus.md](bus.md) | `rtl/` |
| Original RV32I microarchitecture + privileged facilities | Five-stage in-order pipeline; M and U modes, traps, CLINT-style timer, PMP | [isa.md](isa.md), [microarchitecture.md](microarchitecture.md) | `rtl/core/` |
| Executable gate-level model | sky130 standard-cell netlist evaluated from Liberty cell functions | [abstractions.md](abstractions.md) | `scripts/synth_gate.py`, `tools/crates/strata-gate` |
| Every abstraction identified | Model boundary is the `strata_soc` port list: memory macros and board devices only | [abstractions.md](abstractions.md) | `tools/crates/strata-sim` |
| Assembler | Own assembler and linker; clang used only to translate C to assembly | [toolchain.md](toolchain.md) | `tools/crates/strata-asm` |
| Boot software, drivers, OS, game | Boot ROM loads the OS from SPI flash; preemptive kernel with U-mode tasks isolated by PMP | [software.md](software.md) | `sw/` |
| Debugger | Native application over a deterministic gate-level session, with layout and transistor views | [debugger.md](debugger.md) | `tools/crates/strata-debug`, `strata-dbg` |
| Verification | Directed and conformance ISA tests, formal checks, RTL/gate lockstep, equivalence checking, four-state simulation, whole-machine tests | [verification.md](verification.md) | `verif/`, `scripts/` |
| FPGA flow | ULX3S (Lattice ECP5) with yosys, nextpnr, ecppack; board-level simulation with a DVI receiver | [fpga.md](fpga.md) | `rtl/fpga/`, `fpga/`, `scripts/fpga.py` |
| Result categories | Every result is labeled simulated, implementation-tool, or physically tested | [results.md](results.md) | `results/` |
| Transistor-level analysis | ngspice on the PDK's transistor netlists: every cell type's logic, the main cells' timing, any cell from the debugger | [physical.md](physical.md#transistor-level-analysis) | `tools/crates/strata-spice` |
| Physical layout view | OpenROAD place and route of the same netlist, proven equivalent; read by the debugger | [physical.md](physical.md) | `physical/`, `scripts/physical.py`, `tools/crates/strata-layout` |

## 2. System overview

```
                        +-------------------------- strata_soc --------------------------+
  boot flash <-- SPI -->|  +---------+  ibus   +--------+                                 |
                        |  |   S1    |-------->| ibus   |--> ROM port A, RAM port A       |
   terminal <-- UART -->|  |  CPU    |  dbus   +--------+                                 |
                        |  | (RV32I) |-------->| dbus   |--> ROM B, RAM B, VRAM A, FONT A |
    buttons --> GPIO -->|  +---------+         |        |--> SYS UART TIMER GPIO SPI      |
       LEDs <-- GPIO <--|      ^ irq           +--------+    VIDEO INTC (MMIO)            |
    monitor <-- VIDEO <-|      +---- TIMER (mtip, msip), INTC (meip)                      |
                        |  VIDEO scan-out --> VRAM port B, FONT port B                     |
                        +-----------------------------------------------------------------+
```

* One clock, 25 MHz, which is also the 640x480 pixel clock.
* Memories: 4 KiB boot ROM, 64 KiB RAM, 8 KiB text VRAM, 4 KiB font RAM,
  all synchronous dual-port SRAM macros.
* Devices: SYS (ID, halt, reset), UART, TIMER, GPIO, SPI flash controller,
  80x30 text-mode VIDEO, and an interrupt controller.
* Interconnect: two point-to-point decoders; no shared bus and no
  arbitration (see [bus.md](bus.md)).

### 2.1 Processor

S1 implements RV32I with Zicsr and Zifencei, machine and user modes, the
standard trap mechanism, `mcycle`/`minstret`/`time` counters, `WFI`, and
four PMP entries. The subset of the privileged architecture is documented
exactly in [isa.md](isa.md); anything else is an illegal instruction. The
pipeline (F, D, X, M, W) commits all architectural state in W, which makes
every exception precise, including data-bus errors
([microarchitecture.md](microarchitecture.md)).

### 2.2 Software

All guest software uses only the interfaces in [memory-map.md](memory-map.md)
and [isa.md](isa.md).

* **Boot ROM** (assembly): initializes the UART, loads a CRC-checked boot
  image from the SPI flash into RAM and jumps to it.
* **Kernel** (C and assembly, M-mode): trap handling, a preemptive
  round-robin scheduler driven by the timer, system calls, drivers for every
  device, a text console, and PMP-based isolation of user tasks.
* **User programs** (U-mode): a shell on the console and a game.

The toolchain is described in [toolchain.md](toolchain.md) and the software
in [software.md](software.md).

## 3. Modes and fidelity

| Mode | What executes the design | Fidelity | Scope |
|------|--------------------------|----------|-------|
| RTL simulation | Verilator or iverilog on the SystemVerilog | Cycle-exact functional; iverilog is 4-state | Whole machine |
| Gate simulation | `strata-gate` on the sky130 netlist | Cycle-exact, zero-delay, 2-state, cell functions from Liberty | `strata_soc` logic; memories and board devices abstracted ([abstractions.md](abstractions.md)) |
| Transistor analysis | ngspice with sky130 device models on PDK cell netlists | Analog transient, typical corner; schematic netlists without layout parasitics; lumped loads (from the layout's extraction in the debugger) | Every cell type (logic), the most used cells (timing), any selected cell |
| Physical layout | OpenROAD placement, routing and extraction | Real placement and routing geometry and extracted parasitics; router DRC only, no signoff DRC/LVS; typical corner | `strata_soc` logic without memory macros or pads |
| FPGA | ECP5 bitstream on ULX3S | Real hardware; the bitstream is built and the board-level design simulated, not yet run on a board | Whole machine |

[physical.md](physical.md) and [results.md](results.md) give the details and
the evidence for each.

## 4. Debugger

The debugger is a native desktop application built on a deterministic
gate-level simulation session. Its central object is the *inspection
model*, which relates, for any cycle:

* source lines (C or assembly) and instructions, through the debug
  information produced by the assembler;
* retired instructions and the pipeline stage of every instruction in
  flight, read from nets of the netlist;
* bus transactions and memory-macro accesses;
* RTL module instances and the netlist cells and nets inside them;
* for a selected cell, its transistor-level behavior and its location in
  the physical layout.

Replay is deterministic because the only inputs are pin values, which are
recorded with their cycle numbers. Checkpoints are complete state snapshots
(every net, flip-flop, memory and device). Triggers are evaluated every
cycle on any combination of these objects and stop exactly at the cycle on
which they fire. Faults (stuck-at nets, flip-flop upsets, memory bit flips)
are part of the recorded session. The 3D view places the same objects in
space: modules as regions, cells at their placed locations, nets as wires,
and the path of a selected instruction through them.
See [debugger.md](debugger.md).

## 5. Verification

* ISA: directed tests for every instruction and privileged behavior, the
  riscv-tests suite ported to the machine, and riscv-formal checks on the
  core.
* RTL versus netlist: formal equivalence checking where the tools support it,
  and cycle-by-cycle comparison of the retirement trace and all SoC outputs
  between RTL and gate simulation for whole programs.
* Whole machine: scripted boot, shell and game sessions with recorded
  inputs and expected outputs (terminal text, screen captures, halt codes).

See [verification.md](verification.md).

## 6. Path to physical hardware

* FPGA: the same RTL is synthesized for the Lattice ECP5 on a ULX3S board,
  with HDMI video, the on-board flash, buttons, LEDs and USB serial
  ([fpga.md](fpga.md)). Bitstreams for the 25F, 45F and 85F parts meet
  timing, and the complete FPGA design, simulated with a DVI receiver,
  produces the same picture as the gate-level model. It has not yet been
  run on a board; fpga.md gives the bring-up procedure.
* ASIC: the gate-level netlist is placed and routed with OpenROAD in the
  sky130 process ([physical.md](physical.md)). The layout meets timing at
  25 MHz with no routing design rule violations and is formally equivalent
  to the simulated netlist. Memory macros, a pad ring and signoff checks
  would be needed for fabrication.

## 7. Repository layout

| Path | Contents |
|------|----------|
| `docs/` | this document and the specifications |
| `rtl/` | SystemVerilog: `core/`, `soc/`, `periph/`, `mem/`, `fpga/` |
| `sw/` | boot ROM, kernel, user programs, runtime library, tests |
| `tools/` | Rust workspace: assembler, netlist and simulation crates, debugger, layout reader, transistor-level analysis |
| `verif/` | testbenches, ISA tests, formal and equivalence setups |
| `scripts/` | flow drivers (synthesis, tests, reports) |
| `fpga/` | ULX3S constraints (the FPGA top is in `rtl/fpga/`) |
| `physical/` | OpenROAD configuration for the sky130 layout |
| `results/` | recorded results, labeled by category ([results.md](results.md)) |

## 8. Status

Everything in the requirements is implemented. [results.md](results.md)
lists every result by category. In short: the machine, its software and the
debugger work in simulation, with the gate-level and RTL models agreeing
cycle for cycle; the FPGA and layout flows complete with timing met; and
nothing has yet been tested on physical hardware.
