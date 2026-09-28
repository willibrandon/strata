#!/usr/bin/env python3
"""Runs the riscv-formal checks on the S1 core.

Sets up third_party/riscv-formal/cores/strata from verif/formal, generates the
checks with riscv-formal's genchecks.py, runs them with SymbiYosys, and writes
a summary to results/formal/summary.json.

Usage: scripts/formal.py [-j N] [pattern ...]   (patterns filter check names)
"""
import argparse
import fnmatch
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
from env import OSS_CAD  # noqa: E402

RVF = ROOT / "third_party/riscv-formal"
CORE = RVF / "cores/strata"


def setup():
    if not RVF.exists():
        sys.exit("third_party/riscv-formal missing: run scripts/fetch_third_party.sh")
    if CORE.exists():
        shutil.rmtree(CORE)
    CORE.mkdir(parents=True)
    shutil.copy(ROOT / "verif/formal/wrapper.sv", CORE / "wrapper.sv")
    cfg = (ROOT / "verif/formal/checks.cfg").read_text().replace("@rtl@", str(ROOT / "rtl"))
    (CORE / "checks.cfg").write_text(cfg)
    env = dict(os.environ, PATH=f"{OSS_CAD / 'bin'}:{os.environ['PATH']}")
    subprocess.run([sys.executable, "../../checks/genchecks.py"], cwd=CORE, env=env, check=True,
                   stdout=subprocess.DEVNULL)
    return env


def status(check_dir: Path) -> str:
    st = check_dir / "status"
    if st.exists():
        return st.read_text().split()[0]
    return "UNKNOWN"


def main():
    ap = argparse.ArgumentParser(description="Run riscv-formal checks on the S1 core.")
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("patterns", nargs="*", default=["*"])
    args = ap.parse_args()

    env = setup()
    checks_dir = CORE / "checks"
    sbys = sorted(p.stem for p in checks_dir.glob("*.sby"))
    selected = [c for c in sbys if any(fnmatch.fnmatch(c, p) for p in args.patterns)]
    print(f"{len(selected)} of {len(sbys)} checks selected")

    t0 = time.time()
    procs: dict[str, subprocess.Popen] = {}
    pending = list(selected)
    results = {}
    while pending or procs:
        while pending and len(procs) < args.j:
            c = pending.pop(0)
            procs[c] = subprocess.Popen([str(OSS_CAD / "bin/sby"), "-f", f"{c}.sby"], cwd=checks_dir, env=env,
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for c, p in list(procs.items()):
            if p.poll() is not None:
                del procs[c]
                results[c] = status(checks_dir / c)
                print(f"  {results[c]:8s} {c}", flush=True)
        time.sleep(0.2)

    out = ROOT / "results/formal"
    out.mkdir(parents=True, exist_ok=True)
    summary = {
        "category": "simulated (formal model checking)",
        "tool": "riscv-formal + SymbiYosys",
        "config": "verif/formal/checks.cfg",
        "elapsed_s": round(time.time() - t0, 1),
        "results": results,
    }
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    bad = {c: s for c, s in results.items() if s != "PASS"}
    print(f"{len(results) - len(bad)} passed, {len(bad)} failed in {summary['elapsed_s']} s")
    for c, s in bad.items():
        print(f"  {s}: {c}  (see {checks_dir / c})")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
