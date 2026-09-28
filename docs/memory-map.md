# Strata memory map and device interfaces

This document is the **complete software-visible interface** of the Strata
machine. Guest software (boot ROM, kernel, drivers, user programs) may rely on
nothing else. The C definitions in `sw/include/strata/machine.h` are a direct
transcription of this document.

All addresses are physical (there is no virtual memory). The machine is
little-endian. `RO` = read-only, `RW` = read/write, `W1C` = write 1 to clear,
`WO` = write-only (reads return 0).

## Address map

| Region    | Base          | Size        | Fetch | Load | Store | Backing                       |
|-----------|---------------|-------------|:-----:|:----:|:-----:|-------------------------------|
| Boot ROM  | `0x0000_0000` | 4 KiB       | yes   | yes  | fault | memory macro `rom`  (1024x32) |
| RAM       | `0x1000_0000` | 64 KiB      | yes   | yes  | yes   | memory macro `ram`  (16384x32)|
| Text VRAM | `0x2000_0000` | 8 KiB       | fault | yes  | yes   | memory macro `vram` (2048x32) |
| Font RAM  | `0x2001_0000` | 4 KiB       | fault | yes  | yes   | memory macro `font` (1024x32) |
| MMIO      | `0x4000_0000` | 7 x 4 KiB   | fault | yes  | yes   | device registers (logic)      |

Every address not listed is **unmapped**: an instruction fetch raises an
instruction access fault (cause 1), a load raises a load access fault (cause
5), and a store raises a store access fault (cause 7). There is no aliasing:
the decoders compare all 32 address bits.

Stores to the boot ROM raise a store access fault. Instruction fetches from
VRAM, font RAM, or MMIO raise an instruction access fault.

Memory regions (ROM, RAM, VRAM, font RAM) support aligned byte, halfword, and
word accesses. **MMIO registers must be accessed with aligned 32-bit `lw`/`sw`**;
a byte or halfword access to the MMIO region raises an access fault (load or
store as appropriate), and has no side effects. Within an implemented device
page, offsets that name no register read as zero and ignore writes. The pages
`0x4000_7000`-`0x4FFF_FFFF` are unmapped.

Misaligned accesses anywhere raise address-misaligned exceptions (causes 4/6)
before any bus activity; see [isa.md](isa.md).

Reset starts execution at `0x0000_0000` in machine mode.

## Clock and time base

The machine has a single clock, `clk`, nominally **25 MHz**. It is also the
video pixel clock. `mtime` increments once per `clk` cycle, so the timer
frequency is 25 MHz. `SYS.CLK_HZ` reports the nominal frequency.

## MMIO devices

| Page          | Device | Summary                                          |
|---------------|--------|--------------------------------------------------|
| `0x4000_0000` | SYS    | identification, scratch, halt, software reset    |
| `0x4000_1000` | UART   | 8N1 serial port, 8-entry TX and RX FIFOs         |
| `0x4000_2000` | TIMER  | 64-bit `mtime`/`mtimecmp`, software interrupt    |
| `0x4000_3000` | GPIO   | 8 LEDs, 7 buttons with edge capture              |
| `0x4000_4000` | SPI    | SPI master (mode 0) for the boot flash           |
| `0x4000_5000` | VIDEO  | 80x30 text display control and palette           |
| `0x4000_6000` | INTC   | external interrupt pending/enable                |

### SYS (`0x4000_0000`)

| Offset | Name       | Access | Description |
|--------|------------|--------|-------------|
| `0x00` | ID         | RO     | `0x4152_5453` (the bytes "STRA" in memory order) |
| `0x04` | VERSION    | RO     | `0x0001_0000` (major 1, minor 0) |
| `0x08` | CLK_HZ     | RO     | `25_000_000` |
| `0x0C` | RAM_BYTES  | RO     | `65536` |
| `0x10` | SCRATCH    | RW     | 32-bit scratch register, reset 0 |
| `0x20` | HALT       | WO     | Writing value `v` halts the machine: the CPU stops fetching and retiring instructions permanently (until reset), the `halted` output is asserted and `halt_code` = `v[7:0]`. By convention `v = 0` means success. |
| `0x24` | RESET      | WO     | Writing `0x5245_5345` ("RESE") requests a full machine reset (identical to the external reset). Other values are ignored. |

### UART (`0x4000_1000`)

8 data bits, no parity, 1 stop bit, LSB first. Line idles high.

| Offset | Name   | Access | Description |
|--------|--------|--------|-------------|
| `0x00` | DATA   | RW     | Write: push `v[7:0]` to the TX FIFO (ignored if the FIFO is full). Read: pop the RX FIFO; returns `{23'b0, valid, byte}` where `valid` (bit 8) is 1 if a byte was popped, else the read returns 0 and nothing is popped. |
| `0x04` | STATUS | RO/W1C | bit 0 `TX_FULL`; bit 1 `TX_IDLE` (TX FIFO empty and transmitter idle); bit 2 `RX_AVAIL`; bit 3 `RX_OVERRUN` (sticky, W1C: a received byte was dropped because the RX FIFO was full). |
| `0x08` | CTRL   | RW     | bit 0 `RX_IE`: interrupt while `RX_AVAIL`; bit 1 `TX_IE`: interrupt while the TX FIFO is empty. Reset 0. |
| `0x0C` | DIV    | RW     | Bit period in clock cycles, bits [15:0], minimum 4. Reset **25** (1,000,000 baud at 25 MHz). Writes below 4 are stored as 4. |

The receiver samples the line at the middle of each bit (after a start-bit
edge it waits `DIV/2` cycles, checks the line is still low, then samples every
`DIV` cycles). A byte whose stop bit is 0 is discarded (framing error).
The UART interrupt line is `(RX_IE & RX_AVAIL) | (TX_IE & tx_fifo_empty)`.

### TIMER (`0x4000_2000`)

A subset of the RISC-V CLINT, re-based.

| Offset | Name        | Access | Description |
|--------|-------------|--------|-------------|
| `0x00` | MTIME_LO    | RW     | `mtime[31:0]` |
| `0x04` | MTIME_HI    | RW     | `mtime[63:32]` |
| `0x08` | MTIMECMP_LO | RW     | `mtimecmp[31:0]`, reset `0xFFFF_FFFF` |
| `0x0C` | MTIMECMP_HI | RW     | `mtimecmp[63:32]`, reset `0xFFFF_FFFF` |
| `0x10` | MSIP        | RW     | bit 0: machine software interrupt pending (`mip.MSIP`) |

`mtime` increments by one every clock cycle; a write to either half replaces
that half and suppresses the increment in that cycle. `mip.MTIP` is
`mtime >= mtimecmp` (unsigned 64-bit), evaluated every cycle. The CPU's `time`
and `timeh` CSRs read `mtime` directly. Software updating `mtimecmp` without a
spurious interrupt should write `MTIMECMP_HI = 0xFFFF_FFFF`, then `MTIMECMP_LO`,
then `MTIMECMP_HI`.

### GPIO (`0x4000_3000`)

| Offset | Name     | Access | Description |
|--------|----------|--------|-------------|
| `0x00` | LEDS     | RW     | bits [7:0] drive LEDs 0-7 (1 = on). Reset 0. |
| `0x04` | BUTTONS  | RO     | bits [6:0] current (synchronized) button levels, 1 = pressed. |
| `0x08` | EDGES    | RO/W1C | bits [6:0] sticky press events (0 -> 1 transitions). Write 1s to clear. |
| `0x0C` | IRQ_EN   | RW     | bits [6:0] enable an interrupt while the corresponding EDGES bit is set. |

Button bit assignment: 0 `UP`, 1 `DOWN`, 2 `LEFT`, 3 `RIGHT`, 4 `A` (fire 1),
5 `B` (fire 2), 6 `START`. Button inputs pass through a two-flop synchronizer;
there is no hardware debouncing (the driver debounces in software).
The GPIO interrupt line is `|(EDGES & IRQ_EN)`.

### SPI (`0x4000_4000`)

A byte-oriented SPI master, mode 0 (CPOL = 0, CPHA = 0), MSB first, connected to
the boot flash.

| Offset | Name   | Access | Description |
|--------|--------|--------|-------------|
| `0x00` | DATA   | RW     | Write: start an 8-bit transfer of `v[7:0]` (ignored while busy). Read: the last byte received. |
| `0x04` | STATUS | RO     | bit 0 `BUSY`. |
| `0x08` | CTRL   | RW     | bit 0 `CS`: 1 asserts chip select (drives `spi_cs_n` low). bits [15:8] `HALF`: SCK half-period in clock cycles minus one (0 = fastest: SCK = clk/2). bit 16 `DONE_IE`: interrupt when a transfer completes. Reset 0. |
| `0x0C` | EVENTS | RO/W1C | bit 0 `DONE`: sticky, set when a transfer completes. |

### VIDEO (`0x4000_5000`)

The display is 640x480 at 60 Hz (800x525 total, 25 MHz pixel clock),
presented as **80 columns x 30 rows** of 8x16-pixel character cells.

*Text VRAM layout.* The cell at row `r` (0-29) and column `c` (0-79) is the
16-bit halfword at `0x2000_0000 + r*256 + c*2`: bits [7:0] are the character
code, bits [11:8] the foreground color index, bits [15:12] the background
color index. Columns 80-127 of each row are not displayed and may be used as
ordinary memory, as may rows 30-31.

*Font RAM layout.* Glyph `g` (0-255) occupies 16 bytes at
`0x2001_0000 + g*16`; byte `y` is pixel row `y` (top first), bit 7 is the
leftmost pixel. A set bit selects the foreground color. Font RAM is **not**
initialized by hardware; the boot software loads the font.

| Offset         | Name      | Access  | Description |
|----------------|-----------|---------|-------------|
| `0x00`         | CTRL      | RW      | bit 0 `ENABLE` (0 = output black, reset 0); bit 1 `VBLANK_IE`. |
| `0x04`         | STATUS    | RO/W1C  | bit 0 `IN_VBLANK` (RO, current); bit 1 `VBLANK` (sticky, set on the first line of vertical blanking; W1C). |
| `0x08`         | FRAME     | RO      | frame counter, increments at the start of each vertical blank. |
| `0x40`-`0x7C`  | PALETTE[i]| RW      | `i = (offset-0x40)/4`: bits [11:0] = RGB 4:4:4 (`0xRGB`). |

The video interrupt line is `VBLANK_IE & STATUS.VBLANK`.

Reset palette (index: RGB): 0 `000`, 1 `00A`, 2 `0A0`, 3 `0AA`, 4 `A00`, 5 `A0A`,
6 `A50`, 7 `AAA`, 8 `555`, 9 `55F`, 10 `5F5`, 11 `5FF`, 12 `F55`, 13 `F5F`,
14 `FF5`, 15 `FFF`.

Video timing (pixels / lines): horizontal visible 640, front porch 16, sync 96
(negative), back porch 48; vertical visible 480, front porch 10, sync 2
(negative), back porch 33.

### INTC (`0x4000_6000`)

| Offset | Name    | Access | Description |
|--------|---------|--------|-------------|
| `0x00` | PENDING | RO     | current level of each source: bit 0 UART, bit 1 GPIO, bit 2 VIDEO, bit 3 SPI |
| `0x04` | ENABLE  | RW     | enable mask, reset 0 |

`mip.MEIP = |(PENDING & ENABLE)`. Sources are level-sensitive; software
acknowledges an interrupt at the device that raised it.

## Boot flash

The machine boots from a SPI NOR flash (JEDEC `0x03` READ command, 24-bit
address). The boot ROM looks for a Strata boot image at flash offset
`0x0020_0000`. The image format is specified in
[toolchain.md](toolchain.md#boot-image-format).

## Machine pins

These are the signals of the gate-level model boundary (`strata_soc`) other
than memory-macro ports; see [abstractions.md](abstractions.md).

| Pin              | Dir | Description |
|------------------|-----|-------------|
| `clk`            | in  | 25 MHz clock |
| `rst_in`         | in  | asynchronous reset request, active high (synchronized internally) |
| `uart_rx`        | in  | serial input (idle 1) |
| `uart_tx`        | out | serial output (idle 1) |
| `btn[6:0]`       | in  | buttons, 1 = pressed |
| `led[7:0]`       | out | LEDs, 1 = on |
| `spi_cs_n`, `spi_sck`, `spi_mosi` | out | SPI flash |
| `spi_miso`       | in  | SPI flash |
| `vid_r/g/b[3:0]` | out | pixel color (0 during blanking or when disabled) |
| `vid_hs`, `vid_vs` | out | syncs, active low |
| `vid_de`         | out | display enable (visible area) |
| `halted`         | out | machine halted via `SYS.HALT` |
| `halt_code[7:0]` | out | value written to `SYS.HALT` |
