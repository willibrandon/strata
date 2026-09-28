#!/usr/bin/env python3
"""Gate-level synthesis of strata_soc to SkyWater sky130_fd_sc_hd cells.

Produces the netlist used by gate mode, the debugger, equivalence checking and
physical design:

  build/gate/strata_soc.json   Yosys JSON netlist (hierarchical)
  build/gate/strata_soc.v      structural Verilog
  build/gate/gate_stat.txt     cell counts and area

Module hierarchy is preserved, so each cell belongs to the RTL module that
produced it. Registers are never retimed: every flip-flop is one RTL register
bit. Memory macros are outside strata_soc and do not appear.
"""
import argparse
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
from env import tool, liberty_path  # noqa: E402

# Cells that must not appear (the OpenROAD sky130hd platform's list: probe,
# low-power, isolation and level-shifter cells), plus latches, clock gates and
# scan flops, which the design never needs.
DONT_USE = [
    "sky130_fd_sc_hd__probe_*", "sky130_fd_sc_hd__probec_*", "sky130_fd_sc_hd__lpflow_*",
    "sky130_fd_sc_hd__dlclkp_*", "sky130_fd_sc_hd__sdlclkp_*", "sky130_fd_sc_hd__dlx*",
    "sky130_fd_sc_hd__dlrt*", "sky130_fd_sc_hd__sdf*", "sky130_fd_sc_hd__sedf*",
]


def rtl_files():
    return [str(ROOT / l.strip()) for l in (ROOT / "rtl/files.f").read_text().splitlines() if l.strip()]


def script(out: Path, lib: str) -> str:
    du = " ".join(f"-dont_use {c}" for c in DONT_USE)
    return f"""
plugin -i slang
read_liberty -lib {lib}
read_slang --keep-hierarchy {' '.join(rtl_files())} --top strata_soc
hierarchy -check -top strata_soc
synth -top strata_soc
opt_clean
dfflibmap -liberty {lib} {du}
opt
abc -liberty {lib} {du}
opt_clean
setundef -zero
hilomap -singleton -hicell sky130_fd_sc_hd__conb_1 HI -locell sky130_fd_sc_hd__conb_1 LO
opt_clean
check -assert
tee -o {out}/gate_stat.txt stat -liberty {lib}
write_json {out}/strata_soc.json
write_verilog -noexpr -noattr {out}/strata_soc.v
"""


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", default=str(ROOT / "build/gate"))
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    ys = out / "gate.ys"
    ys.write_text(script(out, liberty_path()))
    r = subprocess.run([tool("yosys"), "-q", "-l", str(out / "yosys.log"), "-s", str(ys)])
    if r.returncode != 0:
        sys.exit(f"yosys failed; see {out / 'yosys.log'}")
    print((out / "gate_stat.txt").read_text().split("Chip area")[0][-1500:])


if __name__ == "__main__":
    main()
