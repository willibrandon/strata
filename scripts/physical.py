#!/usr/bin/env python3
"""Physical layout of strata_soc: OpenROAD place and route in sky130.

  physical.py [--skip-flow]

1. Runs OpenROAD-flow-scripts (Docker image openroad/orfs) on the gate-level
   netlist that the simulator runs (build/gate/strata_soc.v), with
   physical/config.mk and physical/strata_soc.sdc: floorplan, placement,
   clock tree, routing, filler, GDS. Results in build/physical/.
2. Checks that the netlist OpenROAD read is structurally identical to the
   simulated one (scripts/netlist_compare.py).
3. Checks that the final routed netlist is logically equivalent to the one
   OpenROAD read (kepler-formal), so the layout implements the simulated
   netlist, including the buffers, resizing and tie cells the flow added.
4. Records area, timing, power and DRC results in results/physical/summary.json
   (implementation-tool results) and copies the layout image to docs/images/.
"""
import argparse
import json
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import netlist_compare  # noqa: E402

IMAGE = "openroad/orfs:latest"
WORK = ROOT / "build/physical"
RES = WORK / "results/sky130hd/strata_soc/base"
REP = WORK / "reports/sky130hd/strata_soc/base"
LOGS = WORK / "logs/sky130hd/strata_soc/base"


def docker(cmd: str, log: Path) -> None:
    r = subprocess.run(["docker", "run", "--rm", "--platform", "linux/amd64", "-v", f"{ROOT}:/work",
                        "-w", "/OpenROAD-flow-scripts/flow", IMAGE, "bash", "-c", cmd],
                       capture_output=True, text=True)
    log.write_text(r.stdout + r.stderr)
    if r.returncode != 0:
        sys.exit(f"docker command failed (log: {log}):\n" + "\n".join((r.stdout + r.stderr).splitlines()[-20:]))


def flow() -> float:
    for d in ("results", "logs", "reports", "objects"):
        shutil.rmtree(WORK / d, ignore_errors=True)
    WORK.mkdir(parents=True, exist_ok=True)
    t0 = time.time()
    docker("source ../env.sh >/dev/null && make DESIGN_CONFIG=/work/physical/config.mk WORK_HOME=/work/build/physical",
           WORK / "flow.log")
    return time.time() - t0


def structural() -> dict:
    a = netlist_compare.flatten_json(ROOT / "build/gate/strata_soc.json")
    b = netlist_compare.parse_verilog(RES / "1_synth_lec.v", netlist_compare.json_directions(ROOT / "build/gate/strata_soc.json"))
    removed_before = set(a[0]) - set(b[0])
    problems = netlist_compare.compare(a, b)
    return {"simulated_cells": len(a[0]), "cells_read_by_openroad": len(b[0]), "differences": len(problems),
            "cells_without_fanout_removed": sorted(removed_before), "first_differences": problems[:10]}


def lec() -> dict:
    cfg = WORK / "lec/synth_vs_final.yml"
    cfg.parent.mkdir(parents=True, exist_ok=True)
    log = WORK / "lec/synth_vs_final.log"
    cfg.write_text(
        "format: verilog\ninput_paths:\n"
        "  - /work/build/physical/results/sky130hd/strata_soc/base/1_synth_lec.v\n"
        "  - /work/build/physical/results/sky130hd/strata_soc/base/6_final_lec.v\n"
        "liberty_files:\n - /OpenROAD-flow-scripts/flow/platforms/sky130hd/lib/sky130_fd_sc_hd__tt_025C_1v80.lib\n"
        "log_file: /work/build/physical/lec/synth_vs_final.log\n")
    docker("/OpenROAD-flow-scripts/tools/install/kepler-formal/bin/kepler-formal --config "
           "/work/build/physical/lec/synth_vs_final.yml", WORK / "lec/kepler.out")
    text = log.read_text()
    size = lambda what: int(m.group(1)) if (m := re.search(rf"size of common {what}: (\d+)", text)) else None  # noqa: E731
    return {"tool": "kepler-formal (combinational equivalence, flip-flops matched by name)",
            "identical": "Circuits are IDENTICAL" in text and "Found difference" not in text,
            "compared_outputs": size("outputs"), "compared_inputs": size("inputs")}


def metrics() -> dict:
    m = json.loads((LOGS / "6_report.json").read_text())
    g = lambda k: m.get(k)  # noqa: E731
    drc = REP / "5_route_drc.rpt"
    drc_count = sum(1 for l in drc.read_text().splitlines() if "violation" in l.lower()) if drc.exists() else None
    classes = {k.split("class:")[1]: v for k, v in m.items() if k.startswith("finish__design__instance__count__class:")}
    return {
        "die_area_um2": g("finish__design__die__area"),
        "core_area_um2": g("finish__design__core__area"),
        "standard_cell_area_um2": g("finish__design__instance__area__stdcell"),
        "utilization": g("finish__design__instance__utilization"),
        "instances": g("finish__design__instance__count"),
        "instances_by_class": classes,
        "clock_period_ns": 40.0,
        "setup_worst_slack_ns": g("finish__timing__setup__ws"),
        "hold_worst_slack_ns": g("finish__timing__hold__ws"),
        "setup_tns_ns": g("finish__timing__setup__tns"),
        "hold_tns_ns": g("finish__timing__hold__tns"),
        "fmax_mhz": round(g("finish__timing__fmax") / 1e6, 2) if g("finish__timing__fmax") else None,
        "clock_skew_ns": g("finish__clock__skew__setup"),
        "max_slew_violations": g("finish__timing__drv__max_slew"),
        "max_cap_violations": g("finish__timing__drv__max_cap"),
        "max_fanout_violations": g("finish__timing__drv__max_fanout"),
        "power_mw": round(g("finish__power__total") * 1000, 3) if g("finish__power__total") else None,
        "ir_drop_worst_mv": round(g("finish__design_powergrid__drop__worst__net:VDD__corner:default") * 1000, 3)
        if g("finish__design_powergrid__drop__worst__net:VDD__corner:default") is not None else None,
        "drc_violations": drc_count,
    }


def main() -> None:
    ap = argparse.ArgumentParser(description="OpenROAD place and route of strata_soc in sky130.")
    ap.add_argument("--skip-flow", action="store_true", help="only check and report existing results")
    args = ap.parse_args()
    if not (ROOT / "build/gate/strata_soc.v").exists():
        sys.exit("build/gate/strata_soc.v missing: run scripts/synth_gate.py first")
    seconds = None
    if not args.skip_flow:
        seconds = round(flow(), 1)
    s = structural()
    print(f"netlist read by OpenROAD vs simulated netlist: {s['differences']} differences "
          f"({len(s['cells_without_fanout_removed'])} cells without fanout removed)")
    e = lec()
    print(f"routed netlist vs netlist read: {'IDENTICAL' if e['identical'] else 'DIFFERENT'} "
          f"({e['compared_outputs']} outputs compared)")
    mt = metrics()
    for k in ("die_area_um2", "utilization", "setup_worst_slack_ns", "hold_worst_slack_ns", "fmax_mhz",
              "max_slew_violations", "max_cap_violations", "drc_violations", "power_mw"):
        print(f"  {k:24s} {mt[k]}")
    img = REP / "final_all.webp.png"
    if img.exists():
        out = ROOT / "docs/images/layout.jpg"
        out.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["sips", "-Z", "900", "-s", "format", "jpeg", "-s", "formatOptions", "82", str(img), "--out", str(out)],
                       capture_output=True)
    ok = s["differences"] == 0 and e["identical"] and mt["drc_violations"] == 0 and (mt["setup_worst_slack_ns"] or -1) >= 0 \
        and (mt["hold_worst_slack_ns"] or -1) >= 0
    res = ROOT / "results/physical"
    res.mkdir(parents=True, exist_ok=True)
    (res / "summary.json").write_text(json.dumps({
        "category": "implementation-tool",
        "note": "OpenROAD-flow-scripts (openroad/orfs image), sky130hd platform, typical corner; reported by the tools, "
                "not measured. strata_soc logic only: the memory macros are outside the block.",
        "flow_seconds": seconds,
        "netlist_check": s,
        "equivalence": e,
        "metrics": mt,
        "passed": ok,
    }, indent=2) + "\n")
    print("PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
