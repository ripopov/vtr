# /// script
# requires-python = ">=3.11"
# dependencies = ["resvg-py==0.5.0", "Pillow==12.1.0"]
# ///
"""Render the canonical SVG: uv run --script tools/generate-icons.py."""

import io
from pathlib import Path
import struct

from PIL import Image
import resvg_py

ASSETS = Path(__file__).resolve().parents[1] / "assets" / "app-icon"
SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
ICO_SIZES = tuple(size for size in SIZES if size <= 256)
# Standard and Retina ICNS representations, each containing lossless PNG data.
ICNS_SIZES = {
    b"icp4": 16, b"icp5": 32, b"icp6": 64,
    b"ic07": 128, b"ic08": 256, b"ic09": 512, b"ic10": 1024,
    b"ic11": 32, b"ic12": 64, b"ic13": 256, b"ic14": 512,
}


def render_icons():
    svg = (ASSETS / "volna.svg").read_text(encoding="utf-8")
    return {
        size: resvg_py.svg_to_bytes(
            svg_string=svg, width=size, height=size, skip_system_fonts=True
        )
        for size in SIZES
    }


def generate():
    pngs = render_icons()
    for size, png in pngs.items():
        (ASSETS / f"volna-{size}.png").write_bytes(png)
    images = {size: Image.open(io.BytesIO(png)) for size, png in pngs.items()}
    images[256].save(
        ASSETS / "volna.ico", format="ICO",
        sizes=[(size, size) for size in ICO_SIZES],
        append_images=[images[size] for size in ICO_SIZES if size != 256],
    )
    chunks = b"".join(
        tag + struct.pack(">I", len(pngs[size]) + 8) + pngs[size]
        for tag, size in ICNS_SIZES.items()
    )
    (ASSETS / "volna.icns").write_bytes(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)
    print(f"Generated PNG, ICO and ICNS icons in {ASSETS}")


if __name__ == "__main__":
    generate()
