# /// script
# requires-python = ">=3.11"
# dependencies = ["resvg-py>=0.2", "pillow>=11"]
# ///
"""Render the logo SVGs to anada.ico and anada.png. Run: uv run assets/build-icon.py

anada-small.svg has thicker lines and is used for the sizes up to 24 pixels.
"""

import io
from pathlib import Path

import resvg_py
from PIL import Image

HERE = Path(__file__).parent
SMALL = (16, 20, 24)
LARGE = (32, 40, 48, 64, 256)


def render(name: str, size: int) -> Image.Image:
    png = resvg_py.svg_to_bytes(svg_path=str(HERE / name), width=size, height=size)
    return Image.open(io.BytesIO(bytes(png))).convert("RGBA")


images = [render("anada-small.svg", size) for size in SMALL]
images += [render("anada.svg", size) for size in LARGE]
images[-1].save(
    HERE / "anada.ico",
    format="ICO",
    sizes=[image.size for image in images],
    append_images=images[:-1],
    bitmap_format="bmp",
)
images[-1].save(HERE / "anada.png", optimize=True)
