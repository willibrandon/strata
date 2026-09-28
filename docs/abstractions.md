# Abstractions in each simulation mode

This document lists every place where something other than the Strata logic
itself stands in for part of the machine. Everything not listed here is
simulated from the design.

## The gate-level model

In **gate mode** the machine is `strata_soc` synthesized to SkyWater
`sky130_fd_sc_hd` standard cells (`scripts/synth_gate.py`, output
`build/gate/strata_soc.json`). It contains the CPU, the register file, the
bus decoders, all seven MMIO devices, the reset synchronizer and the video
timing and scan-out pipeline: 14,900 cells, 3,214 of them flip-flops.

The simulator (`tools/crates/strata-gate`) evaluates each cell output from the
Boolean function in the cell's Liberty definition and updates each flip-flop
from its Liberty `next_state` function at the clock edge. It has no knowledge
of instructions, registers, buses or devices. The instruction set, the
pipeline, the UART protocol and the video timing are all consequences of the
netlist.

The boundary of the model is the port list of `strata_soc`:

```
            +------------------------------ strata_soc (gate netlist) ------------------------------+
 clk ------>|                                                                                        |
 rst_in --->|  S1 core   ibus/dbus decoders   SYS UART TIMER GPIO SPI VIDEO INTC   reset sync        |
 pins <---->|                                                                                        |
            +-----+---------------+----------------+----------------+--------------------+-----------+
                  | rom A/B       | ram A/B        | vram A/B       | font A/B           | trace_* (observation only)
            [A1: memory macros, simulated by the host]
 pins: uart_rx/tx, btn, led, spi_*, vid_*, halted  -> [A2: devices on the board, simulated by the host]
```

### A1. Memory macros

The boot ROM (4 KiB), RAM (64 KiB), text VRAM (8 KiB) and font RAM (4 KiB) are
dual-port synchronous SRAM macros outside the netlist. The host implements
them with exactly the semantics of `rtl/mem/st_sram_dp.sv` (one-cycle read
latency, byte write enables, read-before-write, held read data when not
enabled, port B wins a same-byte write collision). The same Verilog module is
the memory in RTL simulation and is mapped to block RAM by FPGA synthesis
(where one case, a read on one port of the word being written through the
other port in the same cycle, is left undefined; see
[fpga.md](fpga.md#memories)); an ASIC would use SRAM macros with this
interface.

Loading a program image into the ROM or RAM model is the equivalent of
initializing block RAM in an FPGA bitstream. `--ram` preloading is a test
harness shortcut; the real boot path copies the operating system from the
boot flash into RAM with software running on the machine.

### A2. Devices on the pins

The following are parts on the circuit board, modeled by the host at pin level
(`tools/crates/strata-sim/src/devices.rs`). They see only pin values, one
sample per clock cycle, and have no access to the machine's internals.

| Device | Pins | Model |
|--------|------|-------|
| Serial terminal | `uart_tx`, `uart_rx` | Decodes 8N1 frames at a configured bit period (like a terminal set to a baud rate); sends typed bytes as 8N1 frames. |
| Boot flash | `spi_cs_n`, `spi_sck`, `spi_mosi`, `spi_miso` | SPI NOR flash supporting READ (0x03), JEDEC ID (0x9F) and release from power-down (0xAB). `spi_miso` responds within the cycle in which SCK falls, as the real chip does (its output delay is well under a clock period). |
| Buttons | `btn[6:0]` | Levels set by the user or a test script. No bounce is modeled. |
| LEDs | `led[7:0]` | Observed. |
| Display | `vid_r/g/b`, `vid_hs`, `vid_vs`, `vid_de` | Captures pixels using only `vid_de` and the syncs, like a monitor; a frame is complete at the falling edge of `vid_vs`. |

### A3. Time, clock and signal values

* Gate mode is **cycle-based and zero-delay**: combinational logic settles
  instantly between clock edges and every flip-flop samples at the rising
  edge of the single clock. Whether the netlist actually meets timing at
  25 MHz is established separately by static timing analysis (FPGA:
  nextpnr; ASIC: OpenROAD), see [results.md](results.md).
* Values are **two-state** (0/1). There is no X. Power-on flip-flop
  contents are either all zero or pseudo-random from a seed; tests run
  with several seeds to show that the machine does not depend on power-on
  state. Four-state behavior is checked separately by running the RTL in
  iverilog, which flags any X on control outputs after reset.
* `rst_in` is sampled by the two-flop synchronizer in the netlist; outputs
  observed before reset has propagated (the first `reset cycles + 3` cycles)
  are ignored by the host.

## RTL mode

In **RTL mode** the same `strata_soc` RTL is simulated by an RTL simulator
(Verilator, or iverilog through `verif/tb/tb_machine.sv`). Abstractions A1-A3
are identical. RTL mode is faster and is used for long runs; it is not the
gate-level model. Both modes produce the same retirement trace, cycle for
cycle, which is checked by the test suite.

## Views that are not the machine

The debugger can present decoded views of memory, such as the text screen
rendered directly from VRAM and font RAM contents. These are labeled as
memory views. The display window always shows the pixels captured from the
video pins.

## FPGA and ASIC

On the FPGA the memory macros are ECP5 block RAM, the devices are the real
parts on the ULX3S board, and the clock is real. See [fpga.md](fpga.md). The
ASIC layout in [physical.md](physical.md) covers the `strata_soc` logic only;
the memory macros are outside it.
