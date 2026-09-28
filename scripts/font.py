#!/usr/bin/env python3
"""Compiles and previews the Strata 8x16 text-mode font.

The font source (sw/font/strata8x16.font) is plain text with one block per
defined code point:

    # comment
    glyph 0x41 A
    ........
    ...##...
    (16 rows in total, each exactly 8 characters)

A block is a `glyph <code> [name]` header, where <code> is 0x00-0xFF, followed
by 16 pixel rows, top first; '#' is a set pixel and '.' a clear one. The
leftmost character of a row is bit 7 of the row byte, matching the font RAM
layout in docs/memory-map.md (VIDEO). Blank lines and '#' comment lines may
appear between blocks, but not inside one. Code points without a block are
blank.

Usage:
    scripts/font.py                          validate the font
    scripts/font.py --c font.c --bin f.bin   emit a C array / raw font RAM image
    scripts/font.py --png sheet.png          preview sheet of all 256 code points
    scripts/font.py --text 'Hi\\n└─┘' --png sample.png
                                             render text (escapes: \\n \\\\ \\xHH)

In --text, box drawing, shade and other CP437 graphics may be typed as their
Unicode characters (for example ┌ ░ ♥ ↑); they map to their CP437 code points.
"""
import argparse
import re
import struct
import sys
import zlib
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_FONT = ROOT / "sw/font/strata8x16.font"

GLYPHS = 256
WIDTH = 8
HEIGHT = 16

HEADER = re.compile(r"glyph\s+(0[xX][0-9A-Fa-f]+)(?:\s+(.*))?")
# Outside a block, a line that could be a pixel row is reported rather than
# taken as a comment, so a glyph with too many rows cannot pass silently.
PIXEL_ROW = re.compile(rf"[#.]{{{WIDTH}}}")

# Unicode forms of the CP437 graphics at 0x01-0x1F and 0x7F, which Python's
# cp437 codec maps to control characters instead.
CP437_LOW = "☺☻♥♦♣♠•◘○◙♂♀♪♫☼►◄↕‼¶§▬↨↑↓→←∟↔▲▼"
CP437_HOUSE = "⌂"

Color = tuple[int, int, int]

# Preview colors: the machine's reset palette entries 15 (white), 0 (black)
# and 8 (dark gray), plus distinct shades for grid lines and empty cells.
FG: Color = (0xFF, 0xFF, 0xFF)
BG: Color = (0x00, 0x00, 0x00)
LABEL: Color = (0x55, 0x55, 0x55)
GRID: Color = (0x30, 0x30, 0x50)
UNDEFINED: Color = (0x1C, 0x1C, 0x1C)


@dataclass(frozen=True)
class Glyph:
    code: int
    name: str
    rows: tuple[int, ...]  # HEIGHT row bytes, bit 7 = leftmost pixel
    line: int  # line number of the header in the source


class FontError(Exception):
    """The font source is invalid; args[0] holds one message per problem."""


@dataclass
class _Block:
    """A glyph block whose rows are still being read."""

    code: int | None  # None after a header error: the rows are consumed silently
    name: str
    line: int
    rows: list[int] = field(default_factory=list[int])
    valid: bool = True


def parse_font(text: str, source: str) -> dict[int, Glyph]:
    """Parses font source text; raises FontError listing every problem found."""
    glyphs: dict[int, Glyph] = {}
    first_header: dict[int, int] = {}  # code point -> line of its first header
    errors: list[str] = []
    block: _Block | None = None
    lines = text.splitlines()

    def error(line: int, message: str) -> None:
        errors.append(f"{source}:{line}: {message}")

    def unfinished(block: _Block, line: int, where: str) -> None:
        if block.code is not None:
            error(line, f"glyph 0x{block.code:02X} (line {block.line}) has only {len(block.rows)} "
                        f"of {HEIGHT} rows {where}")

    for lineno, raw in enumerate(lines, start=1):
        line = raw.rstrip()

        if block is not None:
            if not line or line.startswith("glyph"):
                unfinished(block, lineno, "before this header" if line else "before this blank line")
                block = None  # the line itself is handled below
            else:
                value = parse_row(line)
                if isinstance(value, str):
                    if block.code is not None:
                        error(lineno, f"glyph 0x{block.code:02X} row {len(block.rows)}: {value}")
                    block.valid = False
                    value = 0
                block.rows.append(value)
                if len(block.rows) == HEIGHT:
                    if block.valid and block.code is not None:
                        glyphs[block.code] = Glyph(block.code, block.name, tuple(block.rows), block.line)
                    block = None
                continue

        if not line:
            continue
        m = HEADER.fullmatch(line)
        if m:
            code = int(m.group(1), 16)
            block = _Block(None, (m.group(2) or "").strip(), lineno)
            if code >= GLYPHS:
                error(lineno, f"code point {m.group(1)} is outside 0x00-0x{GLYPHS - 1:02X}")
            elif code in first_header:
                error(lineno, f"duplicate glyph 0x{code:02X} (first defined at line {first_header[code]})")
            else:
                first_header[code] = lineno
                block.code = code
        elif line.startswith("glyph"):
            error(lineno, "malformed header; expected 'glyph 0xNN [name]'")
            block = _Block(None, "", lineno)
        elif PIXEL_ROW.fullmatch(line.strip()):
            error(lineno, f"pixel row outside a glyph block (does the glyph above have more than {HEIGHT} rows?)")
        elif not line.lstrip().startswith("#"):
            error(lineno, f"unexpected text {line.strip()!r}; expected a comment or 'glyph 0xNN [name]'")

    if block is not None:
        unfinished(block, len(lines), "at the end of the file")
    if errors:
        raise FontError(errors)
    return glyphs


def parse_row(line: str) -> int | str:
    """Returns the row byte, or an error message if the row is malformed."""
    if len(line) != WIDTH:
        return f"expected {WIDTH} characters, found {len(line)} ({line!r})"
    bad = sorted(set(line) - {"#", "."})
    if bad:
        return f"invalid character {', '.join(repr(c) for c in bad)}; use '#' or '.'"
    value = 0
    for ch in line:
        value = (value << 1) | int(ch == "#")
    return value


def load_font(path: Path) -> dict[int, Glyph]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as e:
        raise SystemExit(f"cannot read font: {e}")
    return parse_font(text, display_path(path))


def display_path(path: Path) -> str:
    """The path relative to the repository root when inside it, else as given."""
    try:
        return path.resolve().relative_to(ROOT).as_posix()
    except ValueError:
        return str(path)


def font_image(glyphs: dict[int, Glyph]) -> bytes:
    """The GLYPHS * HEIGHT byte font RAM image; undefined glyphs are blank."""
    image = bytearray(GLYPHS * HEIGHT)
    for code, glyph in glyphs.items():
        image[code * HEIGHT:(code + 1) * HEIGHT] = bytes(glyph.rows)
    return bytes(image)


def c_source(glyphs: dict[int, Glyph], source: str) -> str:
    image = font_image(glyphs)
    lines = [
        "/*",
        f" * Generated by scripts/font.py from {source}. Do not edit.",
        " *",
        " * Strata 8x16 text-mode font: 256 glyphs of 16 bytes. Byte y of a glyph is",
        " * pixel row y (top first) and bit 7 is the leftmost pixel, as expected by",
        " * font RAM at 0x2001_0000 (docs/memory-map.md, VIDEO).",
        " */",
        "",
        "/* Word aligned: the kernel copies it into font RAM 32 bits at a time. */",
        f"const unsigned char strata_font[{GLYPHS} * {HEIGHT}] __attribute__((aligned(4))) = {{",
    ]
    for code in range(GLYPHS):
        row = image[code * HEIGHT:(code + 1) * HEIGHT]
        glyph = glyphs.get(code)
        label = f"0x{code:02X}"
        if glyph and glyph.name:
            label += " " + glyph.name.replace("*/", "* /")
        lines.append(f"    /* {label} */")
        lines.append("    " + " ".join(f"0x{b:02X}," for b in row))
    lines.append("};")
    return "\n".join(lines) + "\n"


class Canvas:
    """An RGB image that can be saved as PNG."""

    def __init__(self, width: int, height: int, color: Color):
        self.width = width
        self.height = height
        self.pixels = bytearray(bytes(color) * (width * height))

    def fill(self, x: int, y: int, w: int, h: int, color: Color) -> None:
        span = bytes(color) * w
        for yy in range(y, y + h):
            start = (yy * self.width + x) * 3
            self.pixels[start:start + len(span)] = span

    def glyph(self, x: int, y: int, rows: tuple[int, ...], scale: int, color: Color) -> None:
        for r, bits in enumerate(rows):
            for c in range(WIDTH):
                if bits & (0x80 >> c):
                    self.fill(x + c * scale, y + r * scale, scale, scale, color)

    def text(self, x: int, y: int, codes: list[int], glyphs: dict[int, Glyph], scale: int,
             color: Color) -> None:
        for i, code in enumerate(codes):
            if code in glyphs:
                self.glyph(x + i * WIDTH * scale, y, glyphs[code].rows, scale, color)

    def png(self) -> bytes:
        stride = self.width * 3
        scanlines = b"".join(b"\x00" + self.pixels[y * stride:(y + 1) * stride]
                             for y in range(self.height))
        header = struct.pack(">IIBBBBB", self.width, self.height, 8, 2, 0, 0, 0)  # 8-bit RGB
        return (b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", header)
                + png_chunk(b"IDAT", zlib.compress(scanlines, 9)) + png_chunk(b"IEND", b""))


def png_chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def render_sheet(glyphs: dict[int, Glyph], scale: int) -> Canvas:
    """All 256 cells in a 16x16 grid; row labels give the high hex digit, column labels the low."""
    cw, ch = WIDTH * scale, HEIGHT * scale
    label_scale = max(1, scale - 1)
    margin = 3 * WIDTH * label_scale  # room for a two-digit row label plus a gap
    top = HEIGHT * label_scale + 4
    canvas = Canvas(margin + 16 * (cw + 1) + 1, top + 16 * (ch + 1) + 1, BG)
    digits = [ord(d) for d in "0123456789ABCDEF"]

    canvas.fill(margin, top, 16 * (cw + 1) + 1, 16 * (ch + 1) + 1, GRID)
    for i in range(16):
        x = margin + 1 + i * (cw + 1) + (cw - WIDTH * label_scale) // 2
        canvas.text(x, 2, [digits[i]], glyphs, label_scale, LABEL)
        y = top + 1 + i * (ch + 1) + (ch - HEIGHT * label_scale) // 2
        canvas.text(WIDTH * label_scale // 2, y, [digits[i], ord("x")], glyphs, label_scale, LABEL)
    for code in range(GLYPHS):
        x = margin + 1 + (code % 16) * (cw + 1)
        y = top + 1 + (code // 16) * (ch + 1)
        glyph = glyphs.get(code)
        canvas.fill(x, y, cw, ch, BG if glyph else UNDEFINED)
        if glyph:
            canvas.glyph(x, y, glyph.rows, scale, FG)
    return canvas


def render_text(lines: list[list[int]], glyphs: dict[int, Glyph], scale: int) -> Canvas:
    """Text as the machine shows it, cells abutting, with a one-cell border."""
    cols = max((len(line) for line in lines), default=0) + 2
    canvas = Canvas(cols * WIDTH * scale, (len(lines) + 2) * HEIGHT * scale, BG)
    for i, line in enumerate(lines):
        canvas.text(WIDTH * scale, (i + 1) * HEIGHT * scale, line, glyphs, scale, FG)
    return canvas


def text_codes(text: str) -> list[list[int]]:
    """Splits --text into lines of code points, resolving escapes and CP437 graphics."""
    lines: list[list[int]] = [[]]
    i = 0
    while i < len(text):
        ch = text[i]
        if ch == "\\" and i + 1 < len(text):
            nxt = text[i + 1]
            if nxt == "n":
                lines.append([])
                i += 2
                continue
            if nxt == "\\":
                lines[-1].append(ord("\\"))
                i += 2
                continue
            hex_digits = text[i + 2:i + 4]
            if nxt == "x" and re.fullmatch(r"[0-9A-Fa-f]{2}", hex_digits):
                lines[-1].append(int(hex_digits, 16))
                i += 4
                continue
        if ch == "\n":
            lines.append([])
        else:
            lines[-1].append(char_code(ch))
        i += 1
    return lines


def char_code(ch: str) -> int:
    if ch in CP437_LOW:
        return CP437_LOW.index(ch) + 1
    if ch == CP437_HOUSE:
        return 0x7F
    try:
        return ch.encode("cp437")[0]
    except UnicodeEncodeError:
        raise SystemExit(f"--text: {ch!r} (U+{ord(ch):04X}) has no CP437 code point; use \\xHH")


def main() -> None:
    ap = argparse.ArgumentParser(description="Validate, compile and preview the Strata 8x16 font.")
    ap.add_argument("-f", "--font", type=Path, default=DEFAULT_FONT,
                    help=f"font source (default: {display_path(DEFAULT_FONT)})")
    ap.add_argument("--c", type=Path, metavar="OUT.c", help="write the font as a C array")
    ap.add_argument("--bin", type=Path, metavar="OUT.bin", help="write the raw 4096-byte font RAM image")
    ap.add_argument("--png", type=Path, metavar="OUT.png",
                    help="write a preview: the glyph sheet, or the --text sample")
    ap.add_argument("--text", help="render this text instead of the glyph sheet (needs --png)")
    ap.add_argument("--scale", type=int, help="pixel scale for --png (default: 3 for the sheet, 2 for text)")
    args = ap.parse_args()

    if args.text is not None and args.png is None:
        ap.error("--text needs --png")
    if args.scale is not None and args.scale < 1:
        ap.error("--scale must be at least 1")

    try:
        glyphs = load_font(args.font)
    except FontError as e:
        messages: list[str] = e.args[0]
        for message in messages:
            print(message, file=sys.stderr)
        sys.exit(f"{len(messages)} error(s) in {display_path(args.font)}")

    if args.c:
        args.c.write_text(c_source(glyphs, display_path(args.font)))
    if args.bin:
        args.bin.write_bytes(font_image(glyphs))
    if args.png:
        if args.text is not None:
            canvas = render_text(text_codes(args.text), glyphs, args.scale or 2)
        else:
            canvas = render_sheet(glyphs, args.scale or 3)
        args.png.write_bytes(canvas.png())
    print(f"{display_path(args.font)}: {len(glyphs)} glyphs defined")


if __name__ == "__main__":
    main()
