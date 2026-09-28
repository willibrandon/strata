#!/usr/bin/env python3
"""Builds and runs instruction-level tests on the Strata machine.

Suites:
  riscv-tests   rv32ui and rv32mi from third_party/riscv-tests, with the
                machine's own environment (verif/riscv-tests/env)
  strata        directed tests in verif/isa (privilege, PMP, traps,
                interrupts, counters, bus faults)

Each test is preprocessed by clang, assembled and linked at RAM base by
strata-as, preloaded into RAM, and run by strata-run; a two-instruction ROM
stub jumps to it. A test passes when the machine halts with code 0.

  isa_tests.py [--engine rtl|gate|lockstep] [--suite NAME] [pattern ...]

The lockstep engine runs the RTL and the gate-level netlist together and
fails the test if any output of strata_soc differs in any cycle.
"""
import argparse
import concurrent.futures
import fnmatch
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
from env import LLVM_BIN  # noqa: E402

BUILD = ROOT / "build/isa"
AS = ROOT / "tools/target/release/strata-as"
RUN = ROOT / "tools/target/release/strata-run"
RVT = ROOT / "third_party/riscv-tests"

# Upstream tests that exercise features this machine does not implement,
# with the reason. They are reported as skipped, not run.
SKIP = {
    "rv32mi-breakpoint": "debug triggers (tselect/tdata) are not implemented",
    "rv32mi-illegal": "exercises S-mode (sret, sfence.vma, satp), which S1 does not implement",
    "rv32mi-ma_fetch": "requires compressed instructions (C extension)",
    "rv32ui-ma_data": "requires misaligned loads and stores, which trap on S1",
}

# Preprocessor definitions for upstream sources: legacy mnemonics, and an
# S-mode CSR that csr.S names in code it only runs when misa.S is set.
UPSTREAM_DEFINES = ["-Dscall=ecall", "-Dsbreak=ebreak", "-Dscounteren=0x106"]


def tests(suite: str) -> list[tuple[str, Path, list[str]]]:
    """(name, source, extra include dirs) for each test in a suite."""
    out = []
    if suite in ("riscv-tests", "all"):
        inc = [str(ROOT / "verif/riscv-tests/env"), str(RVT / "isa/macros/scalar"), str(RVT / "env")]
        for group in ("rv32ui", "rv32mi"):
            for src in sorted((RVT / "isa" / group).glob("*.S")):
                out.append((f"{group}-{src.stem}", src, inc))
    if suite in ("strata", "all"):
        inc = [str(ROOT / "verif/isa"), str(ROOT / "sw/include")]
        for src in sorted((ROOT / "verif/isa").glob("*.S")):
            out.append((f"strata-{src.stem}", src, inc))
    return out


def build(name: str, src: Path, inc: list[str]) -> Path:
    d = BUILD / name
    d.mkdir(parents=True, exist_ok=True)
    pre = d / f"{name}.s"
    cmd = [str(LLVM_BIN / "clang"), "--target=riscv32-unknown-elf", "-march=rv32i_zicsr_zifencei",
           "-E", "-x", "assembler-with-cpp", "-D__riscv_xlen=32"]
    if not name.startswith("strata-"):
        cmd += UPSTREAM_DEFINES
    cmd += [f"-I{i}" for i in inc] + [str(src), "-o", str(pre)]
    subprocess.run(cmd, check=True, capture_output=True, text=True)
    hexf = d / f"{name}.hex"
    r = subprocess.run([str(AS), "--org", "0x10000000", "--hex", str(hexf), "-o", str(d / f"{name}.elf"),
                        "--listing", str(d / f"{name}.lst"), str(pre)], capture_output=True, text=True)
    if r.returncode:
        raise RuntimeError(r.stderr.strip())
    return hexf


def rom_stub() -> Path:
    out = BUILD / "rom_stub.hex"
    BUILD.mkdir(parents=True, exist_ok=True)
    subprocess.run([str(AS), "--org", "0", "--hex", str(out), str(ROOT / "verif/riscv-tests/rom_stub.S")],
                   check=True)
    return out


def run_one(name: str, src: Path, inc: list[str], engine: str, rom: Path, max_cycles: int) -> dict:
    t0 = time.time()
    if name in SKIP:
        return {"test": name, "status": "SKIP", "reason": SKIP[name]}
    try:
        hexf = build(name, src, inc)
    except (RuntimeError, subprocess.CalledProcessError) as e:
        msg = e.stderr if isinstance(e, subprocess.CalledProcessError) else str(e)
        return {"test": name, "status": "BUILD-ERROR", "detail": str(msg)[-400:]}
    trace = hexf.with_suffix(f".{engine}.trace")
    r = subprocess.run([str(RUN), "--engine", engine, "--rom", str(rom), "--ram", str(hexf), "--quiet",
                        "--max-cycles", str(max_cycles), "--trace", str(trace)],
                       capture_output=True, text=True)
    status = "FAIL"
    detail = r.stderr.strip().splitlines()[-1] if r.stderr.strip() else ""
    for line in r.stderr.splitlines():
        if "halted with code" in line:
            code = int(line.split("halted with code")[1].split()[0])
            status = "PASS" if code == 0 else "FAIL"
            detail = f"halt code {code}" + ("" if code == 0 else f" (failing test case {code})")
    if "lockstep mismatch" in r.stderr:
        status, detail = "MISMATCH", r.stderr[r.stderr.index("lockstep mismatch"):][:600]
    elif r.returncode != 0 and status == "PASS":
        status = "FAIL"
    return {"test": name, "status": status, "detail": detail, "seconds": round(time.time() - t0, 2)}


def main() -> None:
    ap = argparse.ArgumentParser(description="Run ISA tests on the Strata machine.")
    ap.add_argument("--engine", default="rtl", choices=["rtl", "gate", "lockstep"])
    ap.add_argument("--suite", default="all", choices=["riscv-tests", "strata", "all"])
    ap.add_argument("--max-cycles", type=int, default=500_000)
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("patterns", nargs="*", default=["*"])
    args = ap.parse_args()

    for tool in (AS, RUN):
        if not tool.exists():
            sys.exit(f"{tool} missing: build tools with 'cargo build --release' in tools/")
    rom = rom_stub()
    selected = [t for t in tests(args.suite) if any(fnmatch.fnmatch(t[0], p) for p in args.patterns)]
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.j) as ex:
        futs = [ex.submit(run_one, n, s, i, args.engine, rom, args.max_cycles) for n, s, i in selected]
        for f in concurrent.futures.as_completed(futs):
            r = f.result()
            results.append(r)
            print(f"  {r['status']:11s} {r['test']:28s} {r.get('detail', r.get('reason', ''))}", flush=True)
    results.sort(key=lambda r: r["test"])
    counts: dict[str, int] = {}
    for r in results:
        counts[r["status"]] = counts.get(r["status"], 0) + 1
    out = ROOT / "results/isa"
    out.mkdir(parents=True, exist_ok=True)
    summary = {"category": "simulated", "engine": args.engine, "suite": args.suite, "counts": counts,
               "results": results}
    (out / f"summary-{args.suite}-{args.engine}.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(" ".join(f"{k}: {v}" for k, v in sorted(counts.items())))
    sys.exit(0 if set(counts) <= {"PASS", "SKIP"} else 1)


if __name__ == "__main__":
    main()
