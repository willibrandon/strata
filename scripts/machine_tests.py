#!/usr/bin/env python3
"""Whole-machine tests: scripted sessions on the complete machine.

Each test boots the machine (boot ROM, flash, kernel, shell) in the gate-level
model, the RTL model, or both in lockstep, drives it through a script in
verif/machine/ (typing on the terminal, pressing buttons, waiting for LED
values), and checks:

  * the halt code;
  * the complete terminal transcript against verif/machine/expected/<test>.txt;
  * every screenshot against verif/machine/expected/<name>.png;
  * in lockstep mode, that the RTL and gate-level models agree on every
    output in every cycle.

  machine_tests.py [--quick] [--update] [-j N] [tests ...]

--quick runs every test on the RTL model. --update rewrites the expected
files from this run (review the diff before committing). Writes
results/machine/summary.json.
"""
import argparse
import json
import re
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUN = ROOT / "tools/target/release/strata-run"
SW = ROOT / "build/sw"
OUT = ROOT / "build/machine"
EXPECTED = ROOT / "verif/machine/expected"
FLASH_IMAGE_SIZE = 0x80000  # flash.bin starts with the boot image area; the store follows


@dataclass
class Test:
    name: str
    script: str
    engine: str             # gate, rtl or lockstep
    halt: int
    max_cycles: int
    flash: str = "default"  # default, store_only, bad_crc, none
    power_on: str = "zero"
    same_as: str = ""       # compare transcript and screenshots with this test's expected files
    doc: str = ""


TESTS = [
    Test("boot", "boot", "gate", 1, 3_000_000, doc="boot from flash to the shell prompt"),
    Test("power_on_1", "boot", "gate", 1, 3_000_000, power_on="1", same_as="boot",
         doc="boot with pseudo-random flip-flop contents at power-on (seed 1)"),
    Test("power_on_2", "boot", "gate", 1, 3_000_000, power_on="2", same_as="boot",
         doc="as power_on_1, seed 2"),
    Test("shell_tour", "shell_tour", "lockstep", 42, 20_000_000,
         doc="shell commands, programs, fault containment and the game benchmark, RTL and gate compared every cycle"),
    Test("multitask", "multitask", "gate", 3, 12_000_000,
         doc="a background task counting on the LEDs while the shell runs"),
    Test("crash", "crash", "gate", 4, 10_000_000, doc="all six kinds of user fault are contained"),
    Test("interrupt", "interrupt", "gate", 9, 20_000_000,
         doc="Ctrl-C stops a foreground program and the game, and discards a line at the prompt"),
    Test("game", "game", "gate", 5, 20_000_000, doc="the game played with the buttons, with screenshots"),
    Test("blocks_bench", "blocks_bench", "rtl", 6, 200_000_000, doc="computer player, 40 pieces"),
    Test("uart_boot", "uart_boot", "rtl", 7, 20_000_000, flash="store_only",
         doc="no kernel in the flash: the kernel is loaded over the serial line"),
    Test("bad_crc", "bad_crc", "rtl", 8, 20_000_000, flash="bad_crc",
         doc="a corrupted kernel in the flash is rejected; recovery over the serial line"),
    Test("no_store", "no_store", "rtl", 0xEE, 20_000_000, flash="none",
         doc="erased flash: the kernel stops with a panic when there is no program store"),
]


def flash_variants() -> dict[str, str]:
    """Flash images for the boot failure tests, derived from build/sw/flash.bin."""
    OUT.mkdir(parents=True, exist_ok=True)
    good = (SW / "flash.bin").read_bytes()
    store_only = b"\xff" * FLASH_IMAGE_SIZE + good[FLASH_IMAGE_SIZE:]
    bad = bytearray(good)
    bad[0x1000] ^= 0x01  # inside the kernel's first segment
    paths = {"store_only": OUT / "flash_store_only.bin", "bad_crc": OUT / "flash_bad_crc.bin"}
    paths["store_only"].write_bytes(store_only)
    paths["bad_crc"].write_bytes(bytes(bad))
    return {"default": str(SW / "flash.bin"), "none": "none", **{k: str(v) for k, v in paths.items()}}


def screenshots(script: str) -> list[str]:
    return [Path(m).name for m in re.findall(r"^screenshot\s+\$OUT/(\S+)", script, re.M)]


def run_test(t: Test, engine: str, flashes: dict[str, str]) -> dict:
    work = OUT / t.name
    work.mkdir(parents=True, exist_ok=True)
    text = (ROOT / f"verif/machine/{t.script}.script").read_text()
    script = work / f"{t.script}.script"
    script.write_text(text.replace("$OUT", str(work)).replace("$SW", str(SW)))
    cmd = [str(RUN), "--engine", engine, "--rom", str(SW / "boot/boot.hex"), "--flash", flashes[t.flash],
           "--script", str(script), "--max-cycles", str(t.max_cycles), "--power-on", t.power_on]
    t0 = time.time()
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True)
    secs = time.time() - t0
    stderr = r.stderr.decode(errors="replace")
    (work / "transcript.txt").write_bytes(r.stdout)
    (work / "stderr.txt").write_text(stderr)
    m = re.search(r"halted with code (\d+) after (\d+) cycles, (\d+) instructions", stderr)
    res = {
        "test": t.name, "engine": engine, "doc": t.doc, "seconds": round(secs, 1),
        "halt_code": int(m.group(1)) if m else None,
        "cycles": int(m.group(2)) if m else None,
        "instructions": int(m.group(3)) if m else None,
        "problems": [],
    }
    if r.returncode != 0:
        res["problems"].append(f"strata-run exited with {r.returncode}: {stderr.strip().splitlines()[-1] if stderr.strip() else ''}")
    if res["halt_code"] != t.halt:
        res["problems"].append(f"halt code {res['halt_code']}, expected {t.halt}")
    if engine == "lockstep" and "mismatch" in stderr.lower():
        res["problems"].append("RTL and gate-level models disagree")
    res["transcript"] = r.stdout
    res["shots"] = {name: (work / name) for name in screenshots(text)}
    for name, p in res["shots"].items():
        if not p.exists():
            res["problems"].append(f"screenshot {name} was not written")
    return res


def check_expected(t: Test, res: dict, update: bool) -> None:
    base = t.same_as or t.name
    exp_txt = EXPECTED / f"{base}.txt"
    if update and not t.same_as:
        EXPECTED.mkdir(parents=True, exist_ok=True)
        exp_txt.write_bytes(res["transcript"])
        for name, p in res["shots"].items():
            if p.exists():
                (EXPECTED / name).write_bytes(p.read_bytes())
        return
    if not exp_txt.exists():
        res["problems"].append(f"no expected transcript {exp_txt.relative_to(ROOT)} (run with --update)")
    elif exp_txt.read_bytes() != res["transcript"]:
        want = exp_txt.read_text(errors="replace").splitlines()
        got = res["transcript"].decode(errors="replace").splitlines()
        line = next((i for i, (a, b) in enumerate(zip(want, got)) if a != b), min(len(want), len(got)))
        res["problems"].append(f"transcript differs from {exp_txt.relative_to(ROOT)} at line {line + 1}")
    for name, p in res["shots"].items():
        exp = EXPECTED / name
        if not exp.exists():
            res["problems"].append(f"no expected screenshot {exp.relative_to(ROOT)} (run with --update)")
        elif p.exists() and exp.read_bytes() != p.read_bytes():
            res["problems"].append(f"screenshot {name} differs from {exp.relative_to(ROOT)}")


def main() -> None:
    ap = argparse.ArgumentParser(description="Whole-machine tests.")
    ap.add_argument("tests", nargs="*", help="test names (default: all)")
    ap.add_argument("--quick", action="store_true", help="run every test on the RTL model")
    ap.add_argument("--update", action="store_true", help="rewrite the expected transcripts and screenshots")
    ap.add_argument("-j", type=int, default=4)
    args = ap.parse_args()

    if not (SW / "flash.bin").exists():
        sys.exit("software not built: run 'make -C sw' first")
    selected = [t for t in TESTS if not args.tests or t.name in args.tests]
    if args.tests and len(selected) != len(args.tests):
        sys.exit(f"unknown test; available: {', '.join(t.name for t in TESTS)}")
    flashes = flash_variants()

    def one(t: Test) -> dict:
        res = run_test(t, "rtl" if args.quick else t.engine, flashes)
        check_expected(t, res, args.update)
        status = "ok" if not res["problems"] else "FAIL"
        print(f"{t.name:14s} {res['engine']:8s} {status:4s} {res['cycles'] or 0:>11,} cycles {res['seconds']:7.1f} s"
              + "".join(f"\n    {p}" for p in res["problems"]), flush=True)
        return res

    # Longest first, so the parallel run finishes sooner.
    order = sorted(selected, key=lambda t: {"lockstep": 0, "gate": 1, "rtl": 2}[t.engine])
    with ThreadPoolExecutor(max_workers=args.j) as ex:
        results = {r["test"]: r for r in ex.map(one, order)}

    failed = [n for n, r in results.items() if r["problems"]]
    summary = {
        "category": "simulated",
        "note": "Whole-machine sessions; expected transcripts and screenshots in verif/machine/expected.",
        "quick": args.quick,
        "tests": [{k: v for k, v in results[t.name].items() if k not in ("transcript", "shots")}
                  for t in selected],
        "passed": len(selected) - len(failed),
        "failed": len(failed),
    }
    if not args.tests and not args.update:
        res_dir = ROOT / "results/machine"
        res_dir.mkdir(parents=True, exist_ok=True)
        (res_dir / ("summary_quick.json" if args.quick else "summary.json")).write_text(json.dumps(summary, indent=2) + "\n")
    print(f"{len(selected) - len(failed)} of {len(selected)} passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
