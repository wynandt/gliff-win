#!/usr/bin/env python3
"""Build assets/gliff.ico from assets/gliff.svg.

Renders the SVG at each ICO size so the small glyphs stay aligned to the
pixel grid. Each size is stored as a PNG inside the ICO, which Windows Vista
and later read correctly (including the 256px image).

Requires cairosvg and Pillow:

    python3 -m pip install cairosvg pillow
    python3 scripts/gen-app-icon.py

The source SVG is the upstream Gliff icon (MIT, Copyright (c) 2026 Kevin
McConnell); see NOTICE and assets/gliff.svg.
"""

from __future__ import annotations

import io
import struct
import sys
from pathlib import Path

SIZES = (16, 20, 24, 32, 40, 48, 64, 256)

ROOT = Path(__file__).resolve().parents[1]
SVG = ROOT / "assets" / "gliff.svg"
ICO = ROOT / "assets" / "gliff.ico"


def require_imports():
    try:
        import cairosvg  # noqa: F401
        from PIL import Image  # noqa: F401
    except ImportError as e:
        sys.exit(
            f"{e}\nInstall the generators with: python3 -m pip install cairosvg pillow"
        )


def render_size(size: int) -> "Image.Image":
    import cairosvg
    from PIL import Image

    png = cairosvg.svg2png(
        url=str(SVG),
        output_width=size,
        output_height=size,
        background_color="rgba(0,0,0,0)",
    )
    image = Image.open(io.BytesIO(png)).convert("RGBA")
    if image.size != (size, size):
        sys.exit(f"{size}px render was {image.size}, expected {size}x{size}")
    # Leave no colour in fully transparent pixels; some viewers treat that as a fringe.
    pixels = image.load()
    for y in range(size):
        for x in range(size):
            r, g, b, a = pixels[x, y]
            if a == 0 and (r or g or b):
                pixels[x, y] = (0, 0, 0, 0)
    return image


def png_bytes(image: "Image.Image") -> bytes:
    buf = io.BytesIO()
    image.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def write_ico(path: Path, images: list[tuple[int, bytes]]) -> None:
    # ICO: ICONDIR + ICONDIRENTRY*N + PNG payloads. width/height 0 means 256.
    count = len(images)
    offset = 6 + 16 * count
    entries = bytearray()
    payloads = bytearray()
    for size, payload in images:
        if size < 1 or size > 256:
            sys.exit(f"ICO size {size} is outside 1..256")
        coded = 0 if size == 256 else size
        entries += struct.pack(
            "<BBBBHHII",
            coded,
            coded,
            0,  # colour count (true colour)
            0,  # reserved
            1,  # planes
            32,  # bit count
            len(payload),
            offset,
        )
        payloads += payload
        offset += len(payload)
    path.write_bytes(struct.pack("<HHH", 0, 1, count) + entries + payloads)


def read_ico_sizes(path: Path) -> list[int]:
    data = path.read_bytes()
    reserved, kind, count = struct.unpack_from("<HHH", data)
    if reserved != 0 or kind != 1:
        sys.exit(f"{path} is not an ICO (reserved={reserved}, type={kind})")
    sizes = []
    for i in range(count):
        width, height, _cc, _res, _planes, _bits, nbytes, off = struct.unpack_from(
            "<BBBBHHII", data, 6 + i * 16
        )
        if width != height:
            sys.exit(f"{path} entry {i} is not square: {width}x{height}")
        size = 256 if width == 0 else width
        payload = data[off : off + nbytes]
        if payload[:8] != b"\x89PNG\r\n\x1a\n":
            sys.exit(f"{path} entry {i} ({size}px) is not a PNG")
        sizes.append(size)
    return sizes


def main() -> None:
    require_imports()

    if not SVG.is_file():
        sys.exit(f"missing {SVG}")
    images = [(size, png_bytes(render_size(size))) for size in SIZES]
    ICO.parent.mkdir(parents=True, exist_ok=True)
    write_ico(ICO, images)
    got = read_ico_sizes(ICO)
    if got != list(SIZES):
        sys.exit(f"wrote {ICO} but it contains sizes {got}, expected {list(SIZES)}")
    print(f"wrote {ICO.relative_to(ROOT)} ({ICO.stat().st_size} bytes), sizes {got}")


if __name__ == "__main__":
    main()
