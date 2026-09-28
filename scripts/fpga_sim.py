#!/usr/bin/env python3
"""Simulates the ULX3S FPGA top and checks its HDMI output.

The board testbench (verif/fpga/tb_ulx3s.sv) runs the whole FPGA design
(PLL model, power-on reset, machine booting from the flash model, DVI
serializer) under Verilator. Its DVI receiver checks every decoded TMDS word
against the machine's video outputs and writes decoded frame N. This script
then produces frame N with strata-run (RTL model, and the gate-level model
unless --no-gate) and requires the three images to be identical.

  fpga_sim.py [--frame N] [--no-gate]

Writes results/fpga/board_sim.json.
"""
import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
from env import tool  # noqa: E402

OUT = ROOT / "build/fpga/sim"
RUN = ROOT / "tools/target/release/strata-run"


def sources() -> list[str]:
    files = [l.strip() for l in (ROOT / "rtl/files.f").read_text().splitlines() if l.strip()]
    files += [
        "rtl/mem/st_sram_dp.sv", "rtl/soc/strata_machine.sv",
        "rtl/fpga/st_tmds_encoder.sv", "rtl/fpga/st_hdmi_out.sv", "rtl/fpga/ulx3s_pll.sv", "rtl/fpga/ulx3s_top.sv",
        "verif/fpga/ecp5_models.sv", "verif/tb/spi_flash_model.sv", "verif/fpga/tb_ulx3s.sv",
    ]
    return files


def build() -> Path:
    OUT.mkdir(parents=True, exist_ok=True)
    cmd = [
        tool("verilator"), "--binary", "--timing", "-O3", "-j", "0",
        "--x-assign", "0", "--x-initial", "0",
        "-Wno-fatal", "-Wno-lint", "-Wno-style",
        "--top-module", "tb_ulx3s", "-Mdir", str(OUT / "obj"), "-o", "tb_ulx3s",
        *sources(),
    ]
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    (OUT / "verilator.log").write_text(r.stdout + r.stderr)
    if r.returncode != 0:
        sys.exit("verilator failed:\n" + "\n".join((r.stdout + r.stderr).splitlines()[-30:]))
    return OUT / "obj/tb_ulx3s"


def read_ppm(path: Path) -> tuple[int, int, bytes]:
    data = path.read_bytes()
    magic = data[:2]
    if magic == b"P3":
        tok = data.split()
        w, h = int(tok[1]), int(tok[2])
        return w, h, bytes(int(t) for t in tok[4:4 + w * h * 3])
    if magic == b"P6":
        tok = data.split(maxsplit=4)
        w, h = int(tok[1]), int(tok[2])
        return w, h, tok[4][: w * h * 3] if len(tok[4]) >= w * h * 3 else data[-w * h * 3:]
    raise ValueError(f"{path}: not a PPM")


def diff(a: tuple[int, int, bytes], b: tuple[int, int, bytes]) -> int:
    if a[:2] != b[:2]:
        return -1
    return sum(1 for i in range(0, len(a[2]), 3) if a[2][i:i + 3] != b[2][i:i + 3])


def main() -> None:
    ap = argparse.ArgumentParser(description="ULX3S board-level simulation with HDMI check.")
    ap.add_argument("--frame", type=int, default=6, help="frame to compare (default 6: shell prompt)")
    ap.add_argument("--no-gate", action="store_true", help="skip the gate-level comparison")
    args = ap.parse_args()

    flash = ROOT / "build/sw/flash.bin"
    if not flash.exists() or not (ROOT / "build/sw/boot/boot.hex").exists():
        sys.exit("software not built: run 'make -C sw' first")
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "flash.hex").write_text("\n".join(f"{b:02x}" for b in flash.read_bytes()) + "\n")

    t0 = time.time()
    exe = build()
    t_build = time.time() - t0

    board_ppm = OUT / f"board_frame{args.frame}.ppm"
    t0 = time.time()
    r = subprocess.run([str(exe), f"+flash={OUT / 'flash.hex'}", f"+frame={args.frame}", f"+ppm={board_ppm}"],
                       cwd=ROOT, capture_output=True, text=True)
    t_sim = time.time() - t0
    (OUT / "tb.log").write_text(r.stdout + r.stderr)
    m = re.search(r"FRAME (\d+) WORDS (\d+) MISMATCHES (\d+) MAX_DISPARITY (\d+) CYCLES (\d+)", r.stdout)
    passed = re.search(r"^PASS$", r.stdout, re.M) is not None
    if not m:
        sys.exit("board simulation did not finish:\n" + "\n".join((r.stdout + r.stderr).splitlines()[-20:]))
    words, mismatches, disparity, cycles = int(m.group(2)), int(m.group(3)), int(m.group(4)), int(m.group(5))
    print(f"board: {words} TMDS words checked against the video outputs, {mismatches} mismatches, "
          f"max data disparity {disparity}, {cycles} cycles ({t_sim:.0f} s)")

    board = read_ppm(board_ppm)
    engines = ["rtl"] if args.no_gate else ["rtl", "gate"]
    compare = {}
    for eng in engines:
        ppm = OUT / f"{eng}_frame{args.frame}.ppm"
        subprocess.run([str(RUN), "--engine", eng, "--rom", "build/sw/boot/boot.hex", "--flash", str(flash),
                        "--frame", str(args.frame), "--screenshot", str(ppm), "--quiet"],
                       cwd=ROOT, check=True, capture_output=True)
        d = diff(board, read_ppm(ppm))
        compare[eng] = d
        print(f"frame {args.frame}: board vs {eng} model: {'identical' if d == 0 else f'{d} pixels differ'}")

    ok = passed and mismatches == 0 and all(v == 0 for v in compare.values())
    res = ROOT / "results/fpga"
    res.mkdir(parents=True, exist_ok=True)
    (res / "board_sim.json").write_text(json.dumps({
        "category": "simulated",
        "note": "ULX3S top (PLL and DDR output models from verif/fpga/ecp5_models.sv) under Verilator; "
                "HDMI decoded by the testbench's DVI receiver.",
        "frame": args.frame,
        "tmds_words_checked": words,
        "tmds_mismatches": mismatches,
        "max_data_disparity": disparity,
        "cycles_after_reset": cycles,
        "frame_pixels_differing": compare,
        "passed": ok,
        "seconds": {"build": round(t_build, 1), "simulation": round(t_sim, 1)},
    }, indent=2) + "\n")
    print("PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
