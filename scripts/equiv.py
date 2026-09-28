#!/usr/bin/env python3
"""Formal equivalence of the RTL and the gate-level netlist, per module.

Both designs keep one module per RTL instance with the same names (for
example `st_alu$strata_soc.u_core.u_alu`): the gold side is the RTL
elaborated by yosys-slang with hierarchy kept (then `opt -full`, which
removes register bits that nothing reads; synthesis removes them too, and
they cannot affect any port), the gate side is
build/gate/strata_soc.v with every sky130 cell's behavior taken from its
Liberty function. Each module's own logic is checked with Yosys's
equiv_make / equiv_induct / equiv_simple, with its submodule instances cut
out (their outputs become free inputs, their inputs checked outputs); each
submodule is checked on its own, so together the checks cover the whole
design. Registers are matched by name and every matched net is proved
equal by 4-step induction (all unproven points serve as the induction
hypothesis, so induction runs first), then any remainder by bounded
sequential SAT.

Writes results/equiv/summary.json.
"""
import argparse
import concurrent.futures
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
from env import liberty_path, tool  # noqa: E402

WORK = ROOT / "build/equiv"


def rtl_files() -> str:
    return " ".join(str(ROOT / l.strip()) for l in (ROOT / "rtl/files.f").read_text().splitlines() if l.strip())


def gate_modules() -> list[str]:
    text = (ROOT / "build/gate/strata_soc.v").read_text()
    # Escaped identifiers ("\\st_alu$strata_soc.u_core.u_alu") lose their backslash.
    return [m.lstrip("\\") for m in re.findall(r"^module\s+(\S+)\s*\(", text, re.M)]


def children() -> dict[str, list[str]]:
    """Module -> types of its submodule instances, from the gate netlist."""
    mods = json.loads((ROOT / "build/gate/strata_soc.json").read_text())["modules"]
    user = {m for m in mods if not m.startswith("sky130_")}
    return {m: sorted({c["type"] for c in mods[m]["cells"].values() if c["type"] in user}) for m in user}


CHILDREN: dict[str, list[str]] = {}


def script(module: str) -> str:
    """Checks one module's own logic: its submodule instances become black
    boxes, matched by instance name in both designs (each submodule is
    checked separately)."""
    kids = CHILDREN.get(module, [])
    # Black-box the children and flatten, which expands the sky130 cells into
    # their Liberty logic; then cut each child instance out, so its outputs
    # become free inputs and its inputs become checked outputs, named
    # identically in both designs.
    cut = " ".join(f"t:{k}" for k in kids)
    flatten = (f"blackbox {' '.join(kids)}\nflatten\nexpose -evert {cut}\n" if kids else "flatten\n")
    esc = module
    return f"""
plugin -i slang
read_slang --keep-hierarchy {rtl_files()} --top strata_soc
hierarchy -top {esc}
prep -top {esc}
memory_map
opt -full
{flatten}opt_clean
rename -top gold
design -stash gold

read_liberty -overwrite -ignore_miss_func {liberty_path()}
read_verilog {ROOT / 'build/gate/strata_soc.v'}
hierarchy -top {esc}
{flatten}opt_clean
rename -top gate
design -stash gate

design -copy-from gold -as gold gold
design -copy-from gate -as gate gate
equiv_make gold gate equiv
hierarchy -top equiv
async2sync
equiv_induct -seq 4
equiv_simple -seq 4
equiv_status
"""


def check(module: str) -> dict:
    safe = re.sub(r"[^A-Za-z0-9_.]", "_", module)
    ys = WORK / f"{safe}.ys"
    log = WORK / f"{safe}.log"
    ys.write_text(script(module))
    t0 = time.time()
    r = subprocess.run([tool("yosys"), "-q", "-l", str(log), "-s", str(ys)], capture_output=True, text=True)
    text = log.read_text() if log.exists() else r.stderr
    m = re.search(r"Of those cells (\d+) are proven and (\d+) are unproven", text)
    proven, unproven = (int(m.group(1)), int(m.group(2))) if m else (0, -1)
    status = "PASS" if r.returncode == 0 and m and unproven == 0 else "FAIL"
    return {"module": module, "status": status, "proven": proven, "unproven": unproven,
            "seconds": round(time.time() - t0, 1), "log": str(log.relative_to(ROOT))}


def main() -> None:
    ap = argparse.ArgumentParser(description="Per-module RTL versus gate-level equivalence.")
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("modules", nargs="*", help="modules to check (default: all)")
    args = ap.parse_args()
    if not (ROOT / "build/gate/strata_soc.v").exists():
        sys.exit("build/gate/strata_soc.v missing: run scripts/synth_gate.py")
    WORK.mkdir(parents=True, exist_ok=True)
    CHILDREN.update(children())
    mods = args.modules or gate_modules()
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.j) as ex:
        for r in ex.map(check, mods):
            results.append(r)
            print(f"  {r['status']}  {r['proven']:6d} proven  {r['unproven']:4d} unproven  "
                  f"{r['seconds']:7.1f} s  {r['module']}", flush=True)
    bad = [r for r in results if r["status"] != "PASS"]
    out = ROOT / "results/equiv"
    out.mkdir(parents=True, exist_ok=True)
    summary = {
        "category": "simulated (formal equivalence checking)",
        "tool": "yosys equiv_make/equiv_simple/equiv_induct",
        "gold": "rtl/ elaborated with yosys-slang",
        "gate": "build/gate/strata_soc.v with sky130_fd_sc_hd Liberty cell functions",
        "modules": len(results),
        "passed": len(results) - len(bad),
        "proven_points": sum(r["proven"] for r in results),
        "results": results,
    }
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"{summary['passed']}/{len(results)} modules equivalent, {summary['proven_points']} points proven")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
