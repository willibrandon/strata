# FPGA implementation

Strata's FPGA target is the [ULX3S](https://github.com/emard/ulx3s) board
(Lattice ECP5 in the CABGA381 package; the 25F, 45F and 85F variants are
supported). The flow uses only open tools: yosys with the yosys-slang
frontend for synthesis, nextpnr-ecp5 for placement, routing and timing, and
ecppack from Project Trellis for the bitstream.

This document describes the board top, the flow, what the results mean, and
how to bring the design up on a board. The results fall into three
categories, listed separately in [Results](#results):

* **simulated**: the board top (everything that goes into the FPGA) run in a
  simulator with models of the ECP5 primitives;
* **implementation-tool**: resource use and timing as reported by yosys and
  nextpnr for a real device;
* **physically tested**: behavior observed on a board. None yet; see
  [Bring-up](#bring-up).

## Board top

`rtl/fpga/ulx3s_top.sv` wraps `strata_machine` (the SoC plus its four memory
macros) for the board:

```
 clk_25mhz --> EHXPLLL --+-- 125 MHz clk_bit ------------------------------+
                         +-- 25 MHz clk ---+                               |
                         +-- lock ---> power-on reset (2^16 cycles)        |
                                           |                               |
            btn[6:0] --> button map --> strata_machine --> vid_* --> st_hdmi_out --> gpdi_dp[3:0]
            ftdi_txd ------------------>  (SoC, ROM,   --> uart_tx --> ftdi_rxd
            flash_miso ---------------->   RAM, VRAM,  --> spi_* --> flash_csn, flash_mosi,
                                           font RAM)        USRMCLK (flash clock)
                                                       --> led (halt code when halted)
```

| Part | File | Notes |
|------|------|-------|
| PLL | `rtl/fpga/ulx3s_pll.sv` | EHXPLLL: 25 MHz in, 125 MHz (CLKOP, feedback) and 25 MHz (CLKOS) out, VCO 625 MHz. Parameters as computed by Trellis' `ecppll`. |
| Power-on reset | `ulx3s_top.sv` | Holds `rst_in` until the PLL has locked and for 2^16 cycles after (2.6 ms). |
| Buttons | `ulx3s_top.sv` | UP, DOWN, LEFT, RIGHT to the machine's direction bits, FIRE1 to A, FIRE2 to B, PWR (active low) to START. |
| LEDs | `ulx3s_top.sv` | `GPIO.LEDS` while running; the halt code once the machine halts. |
| Serial | pins `ftdi_txd`, `ftdi_rxd` | The FTDI USB serial chip. The UART's reset divisor (25) gives 1,000,000 baud, 8N1. |
| Flash | `USRMCLK`, `flash_*` | The configuration flash is the machine's boot flash. After configuration its clock pin is reachable only through the `USRMCLK` primitive; the LPF releases the master SPI port to user logic (`MASTER_SPI_PORT=DISABLE`). |
| DVI | `rtl/fpga/st_hdmi_out.sv`, `st_tmds_encoder.sv` | 640x480 at 60 Hz over the HDMI connector (DVI signaling, no audio or InfoFrames). |

### DVI output

`st_tmds_encoder` is the DVI 1.0 TMDS encoder: transition minimization,
running-disparity DC balancing during active video, and the four control
tokens during blanking (blue carries HSYNC and VSYNC, active low for this
mode). The 4-bit color components are widened to 8 bits by repetition.

`st_hdmi_out` serializes the three 10-bit symbols and the clock channel
pattern (`0000011111`) least significant bit first, two bits per 125 MHz
cycle through `ODDRX1F` DDR output registers. The pins are LVCMOS33D
pseudo-differential outputs, as on all ULX3S designs.

The serializer loads a new symbol every five bit clocks. The load point is
found once after configuration: a bit that toggles every pixel clock is
synchronized into the bit-clock domain, and its first change fixes the
phase of a free-running divide-by-5 counter. The load happens one or two bit
clocks after a pixel-clock edge (depending on how the synchronizer resolved
the first change), never on one, so the symbols have settled. The two clocks
come from the same locked PLL, so the phase does not drift and is not
revisited. nextpnr reports the worst `clk -> clk_bit` path at 1.7 ns, within
the 8 ns that the load point allows.

### Memories

The four `st_sram_dp` instances map to DP16KD block RAM: ROM 2, RAM 32, VRAM 4,
font RAM 2, total 40 of the 56 blocks in the smallest (25F) device. The boot
ROM contents come from `build/sw/boot/boot.hex` through `$readmemh` and are
part of the bitstream. RAM, VRAM and font RAM start as zero, as they do in the
simulation models.

**Difference from the model.** The memory model
([abstractions.md](abstractions.md), A1) specifies that when one port reads a
word in the same cycle as the other port writes it, the read returns the old
contents. ECP5 block RAM does not define the result of that collision. To
honor the model, yosys would build each memory twice (one copy per read
port), which doubles block RAM use and does not fit the 25F. The flow instead
declares the cross-port collision undefined for the four memories
(`RD_COLLISION_X_MASK` in `scripts/fpga.py`), so each maps to a single true
dual-port block. The consequences on the FPGA:

* RAM (port A: instruction fetch, port B: loads and stores). A fetch of the
  word that a store writes in the same cycle may return any value. Only code
  that stores an instruction and then executes it without a `FENCE.I` in
  between can observe this: RISC-V then allows either the old or the new
  instruction, and the FPGA may deliver neither. Strata's `FENCE.I` discards
  everything fetched before it completes, so the kernel's program loader
  (store, then `FENCE.I`) is not affected, and user tasks cannot write their
  own code (PMP, [software.md](software.md)).
* VRAM and font RAM (port A: CPU, port B: video scan-out). A pixel scanned
  out in the same cycle that the CPU writes its character cell may show
  the wrong glyph or color for that one pixel of that one frame.

All other logic is the same RTL that the gate-level model is synthesized
from.

## Flow

```
make -C sw                              # boot ROM, kernel, programs, flash image
python3 scripts/fpga.py --device 85k    # or 45k, 25k
python3 scripts/fpga_sim.py             # board-level simulation with HDMI check
```

`scripts/fpga.py` runs:

1. yosys: `read_slang` of the RTL with the ECP5 cell library as a library
   file (for EHXPLLL, ODDRX1F and USRMCLK), then `synth_ecp5` up to RAM
   mapping, the collision declaration described above, and `synth_ecp5`
   from RAM mapping on. Output `build/fpga/strata_ulx3s.json`.
2. nextpnr-ecp5 for the chosen device, package CABGA381, speed grade 6, with
   `fpga/ulx3s/ulx3s.lpf` (pins from the board's reference constraints) and a
   25 MHz constraint on the oscillator input, from which nextpnr derives the
   PLL output constraints.
3. ecppack with compression. The bitstream must be smaller than 2 MiB so
   that it does not overlap the boot image at `0x200000` in the flash; the
   script checks this.

It writes `results/fpga/ulx3s_<device>.json` with utilization, the maximum
frequency of each clock and the tools' warnings.

`scripts/fpga_sim.py` builds the board testbench `verif/fpga/tb_ulx3s.sv`
with Verilator. The testbench contains the whole FPGA design, behavioral
models of EHXPLLL, ODDRX1F and USRMCLK (`verif/fpga/ecp5_models.sv`), the
flash model loaded with `build/sw/flash.bin`, and a DVI receiver. The
receiver samples the four TMDS lanes at the bit rate, aligns to symbols using
the clock lane and decodes them as a DVI sink does. It checks every decoded
symbol against the machine's video outputs (data, color and sync), in order,
from the first horizontal sync after reset. It also assembles frames with the
same rule as the simulator's display model. The script then renders the same
frame with `strata-run` on the RTL model and on the gate-level model and
requires all three images to be identical.

## Results

### Implementation-tool results

From `results/fpga/ulx3s_<device>.json` (yosys 0.69+154, nextpnr-ecp5 from the
same OSS CAD Suite build, seed 1):

| Device | LUT4 + carry (TRELLIS_COMB) | Flip-flops | DP16KD | 25 MHz clock: fmax | 125 MHz clock: fmax |
|--------|------|------|------|------|------|
| LFE5U-25F | 10,638 / 24,288 (44%) | 3,102 | 40 / 56 | 28.99 MHz | 300.5 MHz |
| LFE5U-45F | 10,638 / 43,848 (24%) | 3,102 | 40 / 108 | 28.62 MHz | 284.2 MHz |
| LFE5U-85F | 10,638 / 83,640 (13%) | 3,102 | 40 / 208 | 28.49 MHz | 236.0 MHz |

Both clocks meet their constraints on all three devices, with 14% to 16%
margin on the system clock. The critical path is the load-to-use forwarding
path: block RAM read data (5.6 ns clock-to-output without the optional
output register) through load alignment, the forwarding multiplexers and
the address adder into the pipeline register that holds the next data
address.

These numbers are what the tools report for the routed design with the
speed grade 6 timing data of Project Trellis. They are not measurements.

### Simulated results

From `results/fpga/board_sim.json`: the board testbench boots the machine
from the flash model to the shell prompt and runs to the end of video frame 6
(2,492,008 pixel clocks after reset). All 2,491,345 decoded TMDS symbols
match the machine's video outputs, the running disparity of the data symbols
stays within 10, and the decoded frame 6 is identical, pixel for pixel, to
frame 6 of the RTL model and of the gate-level model. A deliberately wrong
serializer (green channel carrying red data) fails the check with 779
mismatched symbols and 310 differing pixels.

What this covers: the FPGA-specific RTL (PLL connection, power-on reset,
button and LED mapping, flash clock path, TMDS encoding and serialization)
and its interaction with the machine. What it does not cover: the behavior
of the real PLL and I/O buffers, signal integrity on the HDMI cable, and
whether a particular monitor accepts the signal.

### Physically tested results

None. The design has not yet been run on a ULX3S board. The bitstreams in
`build/fpga/` are the output of the flow above and have not been loaded into
a device.

## Bring-up

With a ULX3S connected over USB (the FTDI port, `US1`):

1. Build: `make -C sw && python3 scripts/fpga.py --device 85k` (use the size
   printed on the FPGA).
2. Program the boot image into the flash (raw data at `0x200000`):

   ```
   openFPGALoader -b ulx3s -f -o 0x200000 build/sw/flash.bin
   ```

3. Load the bitstream into the FPGA's configuration RAM for a first test:

   ```
   openFPGALoader -b ulx3s build/fpga/strata_ulx3s_85k.bit
   ```

4. Open the serial port at 1,000,000 baud, 8N1, for example
   `picocom -b 1000000 /dev/ttyUSB0` (on macOS `/dev/tty.usbserial-*`),
   and connect a monitor to the HDMI (GPDI) connector.
5. Expected: the boot ROM banner, the kernel banner and the `strata>` prompt
   on the serial port; the same text on the monitor. `help` lists the shell
   commands; `blocks` starts the game (buttons: LEFT and RIGHT move, UP or
   FIRE1 rotate, DOWN faster, FIRE2 drop, PWR pause).
6. When this works, write the bitstream to the flash so that the board
   starts Strata at power-on: `openFPGALoader -b ulx3s -f
   build/fpga/strata_ulx3s_85k.bit`. `python3 scripts/fpga.py --program`
   performs steps 2 and 6.

If something does not work:

| Symptom | Meaning |
|---------|---------|
| LEDs show `0x1E` | The boot ROM read an unexpected `SYS.ID`: the MMIO path is broken. |
| LEDs show `0x1F` | A trap during boot; the serial port shows `mcause`, `mepc`, `mtval`. |
| Boot ROM banner, then it waits for an image on the serial port | No valid image at flash `0x200000` (step 2 not done, or the flash could not be read: check that the bitstream was built with `MASTER_SPI_PORT=DISABLE`). The ROM also accepts the kernel image over the serial port (`build/sw/system.img`); the program store is only read from the flash. |
| Nothing on the serial port | Check the port and the baud rate (1,000,000). |
| Serial output but no picture | Monitor compatibility with 25.0 MHz DVI (the VESA mode is 25.175 MHz; nearly all monitors accept 25.0). |

Record results of each step in `results/fpga/` as physically tested results,
separately from the simulated and implementation-tool results above.
