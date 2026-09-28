"""Locations of external tools and PDK files.

Every path can be overridden with an environment variable, so the flows work
with any installation; the defaults match docs/setup.md.
"""
import os
import shutil
from pathlib import Path

HOME = Path.home()
OSS_CAD = Path(os.environ.get("OSS_CAD_SUITE", HOME / "tools/oss-cad-suite"))
PDK_ROOT = Path(os.environ.get("PDK_ROOT", HOME / "tools/pdk"))
LLVM_BIN = Path(os.environ.get("LLVM_BIN", "/opt/homebrew/opt/llvm/bin"))


def tool(name: str) -> str:
    """Path of an EDA tool: OSS CAD Suite first, then PATH."""
    p = OSS_CAD / "bin" / name
    if p.exists():
        return str(p)
    found = shutil.which(name)
    if not found:
        raise SystemExit(f"required tool not found: {name}")
    return found


def sky130() -> Path:
    return PDK_ROOT / "sky130A"


def liberty_path(corner: str = "tt_025C_1v80") -> str:
    return str(sky130() / f"libs.ref/sky130_fd_sc_hd/lib/sky130_fd_sc_hd__{corner}.lib")
