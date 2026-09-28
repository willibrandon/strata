#!/usr/bin/env python3
"""Four-state RTL simulation with Icarus Verilog.

The gate-level model and Verilator are two-state. This runs the RTL of the
whole machine (verif/tb/tb_machine.sv) in iverilog, where uninitialized and
conflicting values are X, and fails if any X reaches the machine's control
outputs or bus handshakes after reset. It runs:

  * every ISA test (riscv-tests and the directed tests), which must halt
    with code 0;
  * a boot from the flash through the boot ROM and kernel to the shell,
    which is told to halt.

  four_state.py [-j N] [--no-boot] [patterns ...]

Writes results/four_state/summary.json.
"""
import argparse
import concurrent.futures
import fnmatch
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
from env import tool  # noqa: E402
import isa_tests  # noqa: E402

OUT = ROOT / "build/four_state"


def compile_tb() -> Path:
    OUT.mkdir(parents=True, exist_ok=True)
    files = [l.strip() for l in (ROOT / "rtl/files.f").read_text().splitlines() if l.strip()]
    files += ["rtl/mem/st_sram_dp.sv", "rtl/soc/strata_machine.sv",
              "verif/tb/spi_flash_model.sv", "verif/tb/tb_machine.sv"]
    vvp = OUT / "tb_machine.vvp"
    r = subprocess.run([tool("iverilog"), "-g2012", "-s", "tb_machine", "-o", str(vvp), *files],
                       cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit("iverilog failed:\n" + r.stderr[-3000:])
    return vvp


def simulate(vvp: Path, plusargs: list[str]) -> tuple[str, float]:
    t0 = time.time()
    r = subprocess.run([tool("vvp"), "-n", str(vvp), *plusargs], cwd=ROOT, capture_output=True, text=True)
    return r.stdout + r.stderr, time.time() - t0


def judge(out: str, want: int) -> tuple[str, str, int | None]:
    if "ERROR: X" in out:
        line = next(l for l in out.splitlines() if "ERROR: X" in l)
        return "X", line.strip(), None
    m = re.search(r"^HALT (\d+) (\d+)", out, re.M)
    if not m:
        return "FAIL", "did not halt" + (" (timeout)" if "TIMEOUT" in out else ""), None
    code, cycles = int(m.group(1)), int(m.group(2))
    return ("PASS" if code == want else "FAIL"), f"halt code {code}", cycles


def isa_test(vvp: Path, rom: Path, name: str, src: Path, inc: list[str]) -> dict:
    if name in isa_tests.SKIP:
        return {"test": name, "status": "SKIP", "reason": isa_tests.SKIP[name]}
    hexf = isa_tests.build(name, src, inc)
    out, secs = simulate(vvp, [f"+rom={rom}", f"+ram={hexf}", "+timeout=500000"])
    status, detail, cycles = judge(out, 0)
    return {"test": name, "status": status, "detail": detail, "cycles": cycles, "seconds": round(secs, 1)}


def boot_test(vvp: Path) -> dict:
    flash = ROOT / "build/sw/flash.bin"
    hexf = OUT / "flash.hex"
    hexf.write_text("\n".join(f"{b:02x}" for b in flash.read_bytes()) + "\n")
    out, secs = simulate(vvp, [f"+rom={ROOT / 'build/sw/boot/boot.hex'}", f"+flash={hexf}",
                               "+uart_in=halt 9\\n", "+uart_delay=2200000", "+timeout=4000000"])
    (OUT / "boot.log").write_text(out)
    status, detail, cycles = judge(out, 9)
    if status == "PASS" and "strata>" not in re.sub(r"\x1b\[[0-9;]*m", "", out):
        status, detail = "FAIL", "no shell prompt"
    return {"test": "boot-to-shell", "status": status, "detail": detail, "cycles": cycles, "seconds": round(secs, 1)}


def main() -> None:
    ap = argparse.ArgumentParser(description="Four-state RTL simulation with iverilog.")
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--no-boot", action="store_true", help="skip the boot from flash (the slowest run)")
    ap.add_argument("patterns", nargs="*", default=["*"])
    args = ap.parse_args()

    vvp = compile_tb()
    rom = isa_tests.rom_stub()
    selected = [t for t in isa_tests.tests("all") if any(fnmatch.fnmatch(t[0], p) for p in args.patterns)]
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.j) as ex:
        futs = [ex.submit(isa_test, vvp, rom, *t) for t in selected]
        if not args.no_boot:
            futs.insert(0, ex.submit(boot_test, vvp))
        for f in concurrent.futures.as_completed(futs):
            r = f.result()
            results.append(r)
            print(f"  {r['status']:5s} {r['test']:28s} {r.get('detail', r.get('reason', ''))}", flush=True)

    counts: dict[str, int] = {}
    for r in results:
        counts[r["status"]] = counts.get(r["status"], 0) + 1
    print("  " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    if args.patterns == ["*"] and not args.no_boot:
        res = ROOT / "results/four_state"
        res.mkdir(parents=True, exist_ok=True)
        (res / "summary.json").write_text(json.dumps({
            "category": "simulated",
            "tool": "Icarus Verilog (four-state), verif/tb/tb_machine.sv",
            "counts": counts,
            "results": sorted(results, key=lambda r: r["test"]),
        }, indent=2) + "\n")
    sys.exit(0 if all(r["status"] in ("PASS", "SKIP") for r in results) else 1)


if __name__ == "__main__":
    main()
