# Strata software

All guest software is built from `sw/` by `make -C sw` (see
[toolchain.md](toolchain.md)). It uses only the interfaces documented in
[memory-map.md](memory-map.md) and [isa.md](isa.md); the C definitions are in
`sw/include/strata/machine.h` and `csr.h`.

## Memory use

| Address | Size | Contents |
|---------|------|----------|
| `0x0000_0000` | 4 KiB | boot ROM (`sw/boot/boot.S`) |
| `0x1000_0000` | 32 KiB | kernel: code, data, console shadow, 2 KiB kernel stack |
| `0x1000_8000` | 12 KiB | slot 0: the shell |
| `0x1000_B000` | 16 KiB | slot 1: the game |
| `0x1000_F000` | 4 KiB | slot 2: utilities (`hello`, `counter`, `crash`) |

The values are defined once, in `sw/include/strata/layout.h`; the build
generates the linker layouts from it.

## Boot flash

| Flash offset | Contents |
|--------------|----------|
| `0x000000` | FPGA bitstream (on the ULX3S) |
| `0x200000` | boot image: the kernel ([format](toolchain.md#boot-image-format)) |
| `0x280000` | program store: user programs |

`scripts/mkflash.py` writes `build/sw/flash.bin`, the flash contents from
`0x200000`.

## Boot ROM

`sw/boot/boot.S` is hand-written assembly (2.4 KiB). At reset it:

1. prints a banner on the UART and checks `SYS.ID`;
2. wakes the flash and issues one READ command at `0x200000`, then streams
   the image header, segment table and segment payloads;
3. checks the header CRC and each segment's CRC-32 (a 256-entry table that the
   assembler computes with `.rept`), and that every segment lies in RAM below
   the 512-byte scratch area the ROM uses;
4. prints the entry point, waits for the UART to drain, executes `FENCE.I`
   and jumps to the kernel in M-mode.

If the flash holds no valid image, the ROM waits for an image in the same
format on the UART (resynchronizing on the magic number), which allows
loading a new kernel without reprogramming the flash. It prints nothing
while the image arrives: the UART has 8-byte FIFOs and no flow control, so
waiting on the transmitter would let received bytes overflow. A trap during boot
prints `mcause`, `mepc` and `mtval` and halts with code `0x1F`.

## Kernel

The kernel (`sw/kernel/`) runs in M-mode with interrupts disabled
(`mstatus.MIE = 0`); user tasks run in U-mode, where the timer and external
interrupts are taken regardless of `MIE`. `mscratch` points at the running
task's register save area and is 0 while the kernel runs, so a trap taken in
the kernel is recognized and reported as a kernel fault.

| File | Contents |
|------|----------|
| `start.S` | entry, `.bss` clearing, trap entry and exit (register save/restore) |
| `main.c` | initialization |
| `trap.c` | trap dispatch, interrupt handling, system calls, panic |
| `task.c` | tasks, round-robin scheduling (10 ms slices), PMP setup, loading |
| `store.c` | the program store in flash |
| `dev.c` | drivers: UART, TIMER, GPIO (software debouncing), VIDEO, SPI flash, INTC |
| `console.c` | 80x30 text console on VRAM mirrored to the UART, ANSI colors |
| `input.c` | keyboard and button input as key codes |

**Tasks.** Each user program is linked for one slot and runs as one task.
A program begins with a header (magic, entry, end of code, end of data, end of
`.bss`, stack top, name; `sw/user/crt0.S`). The kernel copies the program from
the flash into its slot at every start, checks the CRC, zeroes `.bss`, sets
`sp` to the top of the slot and enters it in U-mode with `a0` = the argument
given by the parent.

**Isolation.** Three PMP entries confine a task: TOR `[slot base, end of
code)` readable and executable, TOR `[end of code, top of slot)` readable and
writable, and NAPOT over VRAM and font RAM, readable and writable only while
the task owns the screen. Everything else (the kernel, other slots, devices)
is inaccessible from U-mode. Any fault in a task is reported on the console
and only that task is terminated; `crash <n>` demonstrates six kinds.

**Scheduling.** Round-robin among ready tasks with a 10 ms slice from the
machine timer. Blocking system calls (read, getkey, sleep, vsync, wait)
complete when their event arrives: the kernel fills in the result and makes
the task ready. With nothing to run, the kernel executes `WFI` and handles
the pending interrupt directly.

**Interrupts.** Timer (time slices and sleep), UART receive, button presses
and vertical blanking, through the INTC. Devices are acknowledged at the
device.

## System calls

`ecall` with the call number in `a7` (`sw/include/strata/syscall.h`),
arguments in `a0`-`a2`, result in `a0` (negative for errors).

| Number | Call | Description |
|--------|------|-------------|
| 0 | `exit(code)` | terminate |
| 1 | `write(buf, len)` | write to the console |
| 2 | `read(buf, len, block)` | read a byte of console input |
| 3 | `getkey(block)` | next key code (ASCII or `KEY_*`), or -1 |
| 4 | `sleep(ms)` | block for a time |
| 5 | `time()` | milliseconds since boot |
| 6 | `yield()` | give up the rest of the time slice |
| 7 | `spawn(name, arg)` | start a program from the store; returns its pid |
| 8 | `wait(pid)` | wait for a child to exit; returns its exit code |
| 9 | `kill(pid)` | terminate a task |
| 10 | `ps(buf, max)` | list tasks |
| 11 | `screen(on)` | take or release the text screen; returns the VRAM address |
| 12 | `vsync()` | wait for the next vertical blank; returns the frame number |
| 13 | `palette(i, rgb)` | set a palette entry (screen owner only) |
| 14 | `leds(v)` | set the LEDs |
| 15 | `buttons()` | current button levels |
| 16 | `info(buf)` | cycle and instruction counters, time, flash ID, frame count |
| 17 | `programs(i, name)` | names of the programs in the store |
| 18 | `halt(code)` | stop the machine (used by tests) |
| 19 | `reset()` | reset the machine |
| 20 | `getpid()` | |

## Programs

* **shell**: line editing (Backspace, Ctrl-U, Up arrow to recall the last
  line), Ctrl-C to stop the program running in the foreground (the kernel
  ends it with exit code 130; at the prompt it discards the line), `help`,
  `ls`, `ps`, `kill`, `info` (including
  cycles per instruction from `mcycle`/`minstret`), `leds`, `echo`, `clear`,
  `sleep`, `halt`, `reboot`, and running programs by name, in the foreground
  or with `&` in the background.
* **blocks**: the game (below).
* **hello**, **counter** (`counter &` shows preemptive multitasking on the
  LEDs), **crash** (fault containment).

## Strata Blocks

A falling-block puzzle on the text display. The board is 10 by 20 cells,
each drawn as two full-block characters; a panel shows the next piece,
score, lines and level. Game time is counted in video frames with
`vsync()`, so the game plays at the same pace at any CPU speed. The pieces
come from a 7-bag randomizer.

| Key | Button | Action |
|-----|--------|--------|
| Left, Right | LEFT, RIGHT | move |
| Up, `x` | UP, A | rotate |
| `z` | | rotate back |
| Down | DOWN | faster |
| Space | B | drop |
| `p` | START | pause |
| `q` | | quit |

`blocks demo` lets the computer play (it evaluates every rotation and column
for holes, height, bumpiness and cleared lines). `blocks <n>` places `n`
pieces with the computer player as fast as possible from a fixed seed, prints
the result and exits with the score; the whole-machine tests use it.
