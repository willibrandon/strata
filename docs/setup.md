# Setup

The flows run on macOS and Linux. These are the tools and the versions that
the recorded results were produced with. Locations can be changed with the
environment variables in `scripts/env.py`.

| Tool | Version used | Used for | Location (default) |
|------|--------------|----------|--------------------|
| [OSS CAD Suite](https://github.com/YosysHQ/oss-cad-suite-build) | 2026-09-27 build | yosys 0.69+154 with yosys-slang, Verilator 5.053, Icarus Verilog 14.0 (devel), nextpnr-ecp5 0.11.1, ecppack, openFPGALoader, SymbiYosys and solvers | `~/tools/oss-cad-suite` (`OSS_CAD_SUITE`) |
| SkyWater sky130 PDK | open_pdks `1689ac3f`, installed with [ciel](https://github.com/fossi-foundation/ciel) 3.0.0 | cell library (Liberty, SPICE, GDS), device models | `~/tools/pdk` (`PDK_ROOT`) |
| Rust | 1.98.1 | assembler, simulators, debugger | on `PATH` |
| LLVM clang | 23.1 | translating C to RISC-V assembly (`clang -S`) | `/opt/homebrew/opt/llvm/bin` (`LLVM_BIN`) |
| Python | 3.10 or later (3.14 used) | flow scripts | on `PATH` |
| ngspice | 47 | transistor-level analysis | on `PATH` |
| Docker | 29 | OpenROAD, through the `openroad/orfs` image | on `PATH` |

Only standard Python modules are used.

## Installing

OSS CAD Suite: download the release for your platform and unpack it into
`~/tools/oss-cad-suite` (or set `OSS_CAD_SUITE`).

PDK:

```
python3 -m venv ~/tools/venv
~/tools/venv/bin/pip install ciel
~/tools/venv/bin/ciel enable --pdk-root ~/tools/pdk --pdk-family sky130 1689ac3f2dc763876eaf967227c7dfe831b031ae
```

Rust, LLVM and ngspice (macOS with Homebrew):

```
brew install rust llvm ngspice
```

OpenROAD (the image is built for x86-64; on Apple Silicon Docker runs it
under emulation, which is slower but works):

```
docker pull openroad/orfs:latest
```

Third-party test sources (riscv-tests, riscv-formal), fetched into
`third_party/` at pinned revisions:

```
scripts/fetch_third_party.sh
```

## Building

```
cargo build --release --manifest-path tools/Cargo.toml   # tools/target/release/
make -C sw                                               # build/sw/
python3 scripts/synth_gate.py                            # build/gate/strata_soc.json
```

The software build uses `strata-as` from the first step (and clang from
`/opt/homebrew/opt/llvm/bin` unless given `LLVM=<dir>`). The gate netlist is
needed by the gate simulator and the debugger.
