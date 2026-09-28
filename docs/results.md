# Results

Every result in this repository falls into one of three categories, and each
result file in `results/` states its category:

* **Simulated**: obtained from a model of the design: RTL simulation, the
  gate-level model, transistor-level simulation of cells, or a formal proof
  about the RTL or netlist. It shows what the design does, not what a
  physical device does.
* **Implementation-tool**: reported by the FPGA or ASIC implementation
  tools about a real device or process (resource use, timing, power, design
  rules). It is the tools' analysis, not a measurement.
* **Physically tested**: observed on hardware.

## Simulated

| Result | Numbers | File | Details |
|--------|---------|------|---------|
| ISA conformance (riscv-tests rv32ui, rv32mi) | 54 pass, 4 not applicable (stated reasons) | `results/isa/summary-riscv-tests-*.json` | [verification.md](verification.md#riscv-tests) |
| Directed tests of the privileged architecture | 5 of 5 pass | `results/isa/summary-strata-*.json` | [verification.md](verification.md#directed-tests) |
| ISA tests in lockstep (RTL and gate compared every cycle) | 59 pass, no difference | `results/isa/summary-all-lockstep.json` | |
| riscv-formal checks of the core | 44 of 44 pass (depth 12) | `results/formal/summary.json` | [verification.md](verification.md#formal-checks) |
| RTL-to-netlist equivalence | 21 of 21 modules, 10,211 points proven | `results/equiv/summary.json` | [verification.md](verification.md#formal-equivalence) |
| Four-state simulation (Icarus Verilog) | 60 pass (ISA tests and a boot to the shell), no X | `results/four_state/summary.json` | [verification.md](verification.md#four-state-simulation) |
| Whole-machine sessions | 12 of 12 pass; the 10.82 million cycle shell tour with RTL and gate identical in every cycle | `results/machine/summary.json` | [verification.md](verification.md#whole-machine-tests) |
| FPGA top with DVI receiver | 2,491,345 TMDS symbols match; decoded frame identical to RTL and gate models | `results/fpga/board_sim.json` | [fpga.md](fpga.md#simulated-results) |
| Transistor-level logic of every cell type | 76 cell types, 1,356 output samples, no mismatch with Liberty functions | `results/transistor/verify.json` | [physical.md](physical.md#transistor-level-analysis) |
| Transistor-level timing of the most used cells | 432 arcs: delay 18% below Liberty on average (schematic netlists without parasitics) | `results/transistor/characterize.json` | [physical.md](physical.md#timing-compared-with-liberty) |
| Debugger session semantics | replay, goto, breakpoints, reverse step, faults, inspection model | `cargo test -p strata-debug` | [debugger.md](debugger.md) |

## Implementation-tool

| Result | Numbers | File | Details |
|--------|---------|------|---------|
| ULX3S LFE5U-85F | 10,638 LUT4, 3,102 FF, 40 DP16KD; 28.5 MHz for the 25 MHz clock | `results/fpga/ulx3s_85k.json` | [fpga.md](fpga.md#implementation-tool-results) |
| ULX3S LFE5U-45F | same use; 28.6 MHz | `results/fpga/ulx3s_45k.json` | |
| ULX3S LFE5U-25F | 44% of LUTs, 40 of 56 DP16KD; 29.0 MHz | `results/fpga/ulx3s_25k.json` | |
| sky130 layout of `strata_soc` | see [physical.md](physical.md#results) | `results/physical/summary.json` | [physical.md](physical.md) |

## Physically tested

None yet. No part of the design has been run on an FPGA board or fabricated.
[fpga.md](fpga.md#bring-up) gives the bring-up procedure and what to record.
