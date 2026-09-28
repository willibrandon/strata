# Strata

Strata is a small computer designed from the ground up, together with the
software that runs on it and a debugger that shows how that software runs on
the hardware, down to the logic gates and transistors.

The hardware is written in SystemVerilog. It has a 32-bit RISC-V processor
(RV32I with machine and user modes, traps, a timer and memory protection),
64 KiB of RAM, a boot ROM, a text-mode video controller with 640x480 output,
a serial port, an SPI flash controller, buttons and LEDs. The processor is a
new five-stage pipeline written for this project, not an existing core.

The software is also written for this project: an assembler and linker, a
boot ROM that loads the system from flash, a small preemptive operating
system with a shell, and a falling-block game. C code is compiled to
assembly with clang; everything after that is done by Strata's own tools.

The design is synthesized to about 15,000 standard cells of the SkyWater
130 nm library. The gate-level simulator runs that netlist directly. It
evaluates each cell from its library definition and has no knowledge of
instructions, so everything the computer does in this mode, from booting to
running the game, is produced by the netlist.

## What works

Simulation. The machine boots and runs its operating system, shell and game
in the gate-level model and in RTL simulation, and the two agree on every
output in every cycle. It passes the RISC-V conformance tests that apply to
it and a set of directed tests of its privileged architecture. Formal checks
cover the instruction set, and every module of the netlist is proven
equivalent to its RTL. Whole-machine tests run complete sessions and compare
the terminal output and the screen with expected results.

FPGA. The design builds for the ULX3S board (Lattice ECP5) with open tools
and meets timing on the 25F, 45F and 85F parts. A simulation of the complete
FPGA design, including the HDMI output, produces the same picture as the
gate-level model. The design has not yet been run on a physical board.

Layout. The gate-level netlist is placed and routed in the SkyWater process
with OpenROAD. The result has no routing design rule violations, meets
timing at 25 MHz, and is formally equivalent to the simulated netlist.

Transistors. Every cell type in the netlist has been simulated at transistor
level with ngspice and the foundry models, and computes the logic function
the gate-level model uses for it.

docs/results.md lists every result and says which of them come from
simulation, which from implementation tools, and which from hardware.

## Repository

    rtl/        SystemVerilog: processor, SoC, peripherals, memories, FPGA top
    sw/         boot ROM, kernel, user programs, runtime library
    tools/      Rust: assembler, simulators, debugger, layout and SPICE tools
    verif/      testbenches, ISA tests, formal setup, whole-machine sessions
    scripts/    flow drivers: synthesis, tests, FPGA, layout
    fpga/       ULX3S constraints
    physical/   OpenROAD configuration for the layout
    docs/       design documents
    results/    recorded results

## Getting started

docs/setup.md lists the tools and versions. With them installed:

    scripts/fetch_third_party.sh
    cargo build --release --manifest-path tools/Cargo.toml
    make -C sw
    python3 scripts/synth_gate.py

Run the machine with your terminal as its serial console, and type into its
shell:

    tools/target/release/strata-run --interactive

This uses the gate-level model, which simulates every cell, so the boot
takes about a minute before the strata> prompt appears. Type help for the
commands. Ctrl-C or Ctrl-] quits. The RTL model boots in under a second:

    tools/target/release/strata-run --engine rtl --interactive

Time inside the machine is simulated time. On the RTL model the machine runs
about 8 times slower than real hardware would, and on the gate-level model
several hundred times slower, so a command like sleep 1000 takes a while.
The game draws on the video output, which a terminal cannot show. The
debugger shows it in its Display tab.

To send a fixed list of commands and exit, for scripts and tests, use
--uart-in instead. Each line is typed after the machine prints a prompt:

    tools/target/release/strata-run --engine rtl --uart-in 'blocks 20\nhalt 0\n'

Open the debugger:

    tools/target/release/strata-dbg

## Tests

    python3 scripts/isa_tests.py --engine lockstep
    python3 scripts/formal.py
    python3 scripts/equiv.py
    python3 scripts/four_state.py
    python3 scripts/machine_tests.py
    python3 scripts/fpga_sim.py
    cargo test --release --manifest-path tools/Cargo.toml

docs/verification.md describes what each of these checks.

## Implementation

    python3 scripts/fpga.py --device 85k      # ULX3S bitstream
    python3 scripts/physical.py               # sky130 layout (needs Docker)
    tools/target/release/strata-spice verify  # transistor-level cell check

docs/fpga.md explains how to program a board and what to expect from it.

## Documentation

    docs/design.md              requirements, architecture, and where each part is
    docs/isa.md                 the instruction set and privileged architecture
    docs/microarchitecture.md   the pipeline
    docs/memory-map.md          addresses, registers of every device, pins
    docs/bus.md                 the internal bus protocol
    docs/software.md            boot ROM, kernel, system calls, programs
    docs/toolchain.md           assembler, linker, image formats
    docs/abstractions.md        what each simulation mode models and what it does not
    docs/debugger.md            the debugger
    docs/verification.md        tests and proofs
    docs/fpga.md                the FPGA flow and board bring-up
    docs/physical.md            layout and transistor-level analysis
    docs/results.md             all results by category
    docs/setup.md               tools and installation
