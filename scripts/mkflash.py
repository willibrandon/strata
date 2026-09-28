#!/usr/bin/env python3
"""Builds the boot flash contents: the boot image, then the program store.

The output starts at flash offset FLASH_IMAGE_OFFSET (0x200000). The program
store (format in sw/include/strata/layout.h) is placed at STORE_OFFSET
(0x280000), i.e. 0x80000 bytes into the output.

  mkflash.py -o flash.bin --image system.img name=prog.bin@load_addr:slot_size ...
"""
import argparse
import struct
import sys
import zlib
from pathlib import Path

FLASH_IMAGE_OFFSET = 0x200000
STORE_OFFSET = 0x280000
STORE_MAGIC = 0x52545350
PROG_MAGIC = 0x474F5250
STORE_MAX = 16
ENTRY = struct.Struct("<16sIIII")


def parse_program(spec: str) -> tuple[str, Path, int, int]:
    try:
        name, rest = spec.split("=", 1)
        path, where = rest.rsplit("@", 1)
        addr, size = where.split(":")
        return name, Path(path), int(addr, 0), int(size, 0)
    except ValueError:
        sys.exit(f"mkflash: bad program spec {spec!r} (want name=file.bin@addr:size)")


def main() -> None:
    ap = argparse.ArgumentParser(description="Build the Strata boot flash contents.")
    ap.add_argument("-o", "--output", required=True)
    ap.add_argument("--image", required=True, help="boot image from strata-pack")
    ap.add_argument("programs", nargs="+", help="name=file.bin@load_addr:slot_size")
    args = ap.parse_args()

    image = Path(args.image).read_bytes()
    store_base = STORE_OFFSET - FLASH_IMAGE_OFFSET
    if len(image) > store_base:
        sys.exit(f"mkflash: boot image ({len(image)} bytes) overlaps the program store")

    progs = [parse_program(p) for p in args.programs]
    if len(progs) > STORE_MAX:
        sys.exit(f"mkflash: at most {STORE_MAX} programs")
    header_len = 8 + ENTRY.size * len(progs)
    blobs = bytearray()
    entries = bytearray()
    for name, path, addr, size in progs:
        data = path.read_bytes()
        if len(name.encode()) > 15:
            sys.exit(f"mkflash: program name {name!r} longer than 15 bytes")
        if len(data) > size:
            sys.exit(f"mkflash: {name} is {len(data)} bytes; its slot holds {size}")
        if len(data) < 4 or struct.unpack_from("<I", data)[0] != PROG_MAGIC:
            sys.exit(f"mkflash: {path} does not start with a program header")
        data += b"\0" * (-len(data) % 4)
        offset = header_len + len(blobs)
        entries += ENTRY.pack(name.encode(), offset, addr, len(data), zlib.crc32(data))
        blobs += data
        print(f"  {name:10s} {len(data):6d} bytes at 0x{addr:08x} (slot {size} bytes)")

    store = struct.pack("<II", STORE_MAGIC, len(progs)) + entries + blobs
    out = bytearray(image) + b"\xff" * (store_base - len(image)) + store
    Path(args.output).write_bytes(out)
    print(f"mkflash: {args.output}: boot image {len(image)} bytes, store {len(store)} bytes")


if __name__ == "__main__":
    main()
