#!/usr/bin/env python3
"""FPGA implementation flow for the ULX3S (Lattice ECP5).

  yosys (synth_ecp5) -> nextpnr-ecp5 (place, route, timing) -> ecppack

Outputs in build/fpga/: strata_ulx3s.json (netlist), strata_ulx3s_<device>.config
(routed design), strata_ulx3s_<device>.bit (bitstream), and logs. Writes results/fpga/summary.json with the
resource use and timing reported by the tools (results/fpga/ulx3s_<device>.json). These are implementation-tool
results: they say the design fits and meets timing according to the tools,
not that it has run on a board.

  fpga.py [--device 85k|45k|25k] [--seed N] [--program]

--program writes the bitstream to the board's flash with openFPGALoader,
followed by the software image at FLASH_IMAGE_OFFSET.
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

OUT = ROOT / "build/fpga"
FLASH_IMAGE_OFFSET = 0x200000

# The memory macros (docs/abstractions.md, A1). Their model defines a read on
# one port during a write to the same word on the other port as returning the
# old word. ECP5 block RAM leaves that collision undefined, and yosys would
# keep a second copy of each memory to honor it. The flow declares the
# cross-port collision undefined for these memories (RD_COLLISION_X_MASK), so
# each maps to one block RAM; docs/fpga.md lists this as an FPGA difference.
MACROS = ("u_rom", "u_ram", "u_vram", "u_font")


def rtl_files() -> list[str]:
    files = [str(ROOT / l.strip()) for l in (ROOT / "rtl/files.f").read_text().splitlines() if l.strip()]
    files += [str(ROOT / p) for p in (
        "rtl/mem/st_sram_dp.sv", "rtl/soc/strata_machine.sv",
        "rtl/fpga/st_tmds_encoder.sv", "rtl/fpga/st_hdmi_out.sv", "rtl/fpga/ulx3s_pll.sv", "rtl/fpga/ulx3s_top.sv",
    )]
    return files


def run(cmd: list[str], log: Path) -> subprocess.CompletedProcess:
    r = subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT)
    log.write_text(r.stdout + r.stderr)
    if r.returncode != 0:
        tail = "\n".join((r.stdout + r.stderr).splitlines()[-25:])
        sys.exit(f"{Path(cmd[0]).name} failed (log: {log}):\n{tail}")
    return r


def yosys(script: str, name: str) -> None:
    ys = OUT / f"{name}.ys"
    ys.write_text(script)
    run([tool("yosys"), "-q", "-l", str(OUT / f"{name}.log"), "-s", str(ys)], OUT / f"{name}.out")


def collision_masks(rtlil: Path) -> dict[str, str]:
    """RD_COLLISION_X_MASK values that mark every read/write port pair of
    each macro memory as a don't-care collision, sized for its port counts."""
    masks = {}
    cell = None
    params: dict[str, int] = {}
    for line in rtlil.read_text().splitlines():
        line = line.strip()
        if line.startswith("cell $mem_v2 "):
            cell, params = line.split()[2].lstrip("\\"), {}
        elif cell and line.startswith("parameter \\") and line.split()[1] in ("\\RD_PORTS", "\\WR_PORTS"):
            params[line.split()[1][1:]] = int(line.split()[2])
        elif cell and line == "end":
            if cell in {f"u_machine.{m}.mem" for m in MACROS} and params.get("WR_PORTS"):
                n = params["RD_PORTS"] * params["WR_PORTS"]
                masks[cell] = f"{n}'b" + "1" * n
            cell = None
    return masks


def synth(rom_hex: Path) -> dict:
    share = Path(tool("yosys")).resolve().parent.parent / "share/yosys"
    pre = OUT / "pre_ram.il"
    yosys(f"""
plugin -i slang
read_slang -v {share / 'ecp5/cells_bb.v'} {' '.join(rtl_files())} --top ulx3s_top -G ROM_INIT="{rom_hex}"
hierarchy -top ulx3s_top
synth_ecp5 -top ulx3s_top -run begin:map_ram
select ulx3s_top
write_rtlil -selected {pre}
""", "synth1")
    masks = collision_masks(pre)
    if len(masks) != 3:
        sys.exit(f"expected three writable macro memories, found {sorted(masks)}")
    setparams = "\n".join(f"setparam -set RD_COLLISION_X_MASK {v} ulx3s_top/{k}" for k, v in masks.items())
    yosys(f"""
read_verilog -lib -specify {share / 'lattice/cells_sim_ecp5.v'} {share / 'lattice/cells_bb_ecp5.v'}
read_rtlil {pre}
{setparams}
synth_ecp5 -top ulx3s_top -run map_ram: -json {OUT / 'strata_ulx3s.json'}
tee -o {OUT / 'synth_stat.txt'} stat
""", "synth2")
    stat = (OUT / "synth_stat.txt").read_text()
    cells = {}
    for m in re.finditer(r"^\s+(\d+)\s+(?:[\d.E+-]+\s+)?(\S+)$", stat, re.M):
        cells[m.group(2)] = int(m.group(1))
    return cells


def pnr(device: str, seed: int) -> dict:
    report = OUT / f"nextpnr_{device}.json"
    run([
        tool("nextpnr-ecp5"), f"--{device}", "--package", "CABGA381", "--speed", "6",
        "--json", str(OUT / "strata_ulx3s.json"), "--lpf", str(ROOT / "fpga/ulx3s/ulx3s.lpf"),
        "--textcfg", str(OUT / f"strata_ulx3s_{device}.config"), "--report", str(report),
        "--seed", str(seed), "--timing-allow-fail",
    ], OUT / f"nextpnr_{device}.log")
    return json.loads(report.read_text())


def warnings(log: Path) -> list[str]:
    return sorted({l.split("Warning:", 1)[1].strip() for l in log.read_text().splitlines() if "Warning:" in l})


def main() -> None:
    ap = argparse.ArgumentParser(description="Strata FPGA flow for the ULX3S.")
    ap.add_argument("--device", default="85k", choices=["85k", "45k", "25k"])
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--program", action="store_true", help="write bitstream and software to the board's flash")
    args = ap.parse_args()

    rom_hex = ROOT / "build/sw/boot/boot.hex"
    if not rom_hex.exists():
        sys.exit("build/sw/boot/boot.hex missing: run 'make -C sw' first")
    OUT.mkdir(parents=True, exist_ok=True)
    t0 = time.time()
    cells = synth(rom_hex)
    t_synth = time.time() - t0
    rep = pnr(args.device, args.seed)
    t_pnr = time.time() - t0 - t_synth
    bit = OUT / f"strata_ulx3s_{args.device}.bit"
    run([tool("ecppack"), "--compress", str(OUT / f"strata_ulx3s_{args.device}.config"), str(bit)],
        OUT / "ecppack.log")

    if bit.stat().st_size >= FLASH_IMAGE_OFFSET:
        sys.exit(f"{bit} is {bit.stat().st_size} bytes: it would overlap the boot image at {FLASH_IMAGE_OFFSET:#x}")

    fmax = rep.get("fmax", {})
    util = rep.get("utilization", {})
    clocks = {}
    for name, v in fmax.items():
        key = "clk_bit" if "bit" in name or "CLKOP" in name else "clk"
        clocks[name] = {"achieved_mhz": round(v["achieved"], 2), "constraint_mhz": round(v["constraint"], 2),
                        "met": v["achieved"] >= v["constraint"], "domain": key}
    summary = {
        "category": "implementation-tool",
        "note": "Reported by yosys and nextpnr-ecp5; not measured on hardware.",
        "board": "ULX3S", "device": f"LFE5U-{args.device.upper()}F", "package": "CABGA381", "speed": 6,
        "seed": args.seed,
        "clocks": clocks,
        "utilization": {k: {"used": v["used"], "available": v["available"]} for k, v in util.items() if v["used"]},
        "synth_cells": {k: v for k, v in cells.items() if k.startswith(("TRELLIS", "LUT", "CCU2", "DP16", "MULT", "EHX", "ODDR", "USR"))},
        "bitstream": str(bit.relative_to(ROOT)),
        "warnings": warnings(OUT / f"nextpnr_{args.device}.log"),
        "seconds": {"synthesis": round(t_synth, 1), "place_and_route": round(t_pnr, 1)},
    }
    res = ROOT / "results/fpga"
    res.mkdir(parents=True, exist_ok=True)
    (res / f"ulx3s_{args.device}.json").write_text(json.dumps(summary, indent=2) + "\n")
    for name, c in clocks.items():
        print(f"  {name}: {c['achieved_mhz']} MHz achieved, {c['constraint_mhz']} MHz required ({'met' if c['met'] else 'NOT met'})")
    for k, v in summary["utilization"].items():
        print(f"  {k:14s} {v['used']:6d} / {v['available']}")
    print(f"bitstream: {summary['bitstream']}")

    if args.program:
        loader = tool("openFPGALoader")
        subprocess.run([loader, "-b", "ulx3s", "-f", str(bit)], check=True)
        flash = ROOT / "build/sw/flash.bin"
        subprocess.run([loader, "-b", "ulx3s", "-f", "-o", hex(FLASH_IMAGE_OFFSET), str(flash)], check=True)


if __name__ == "__main__":
    main()
