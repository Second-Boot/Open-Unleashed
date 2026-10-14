#!/usr/bin/env python3
"""Render the application icon from icon.svg into the files the builds embed (they are committed, so a build needs
none of this): icon.icns (the macOS app bundle), icon.ico (the Windows executable), icon-256.png (the window icon on
Windows / Linux, and the icon beside the Linux binary).

    python3 -m venv /tmp/icons && /tmp/icons/bin/pip install resvg-py pillow
    /tmp/icons/bin/python packaging/brand/render_icons.py
"""
import io
import os

import resvg_py
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))


def render(svg, size):
    png = resvg_py.svg_to_bytes(svg_string=svg, width=size, height=size)
    return Image.open(io.BytesIO(bytes(png))).convert("RGBA")


def main():
    svg = open(os.path.join(HERE, "icon.svg")).read()
    big = render(svg, 1024)
    big.save(os.path.join(HERE, "icon.icns"), sizes=[(s, s) for s in (16, 32, 64, 128, 256, 512, 1024)])
    # each .ico size rendered from the vector, not scaled down from one bitmap
    ico = [render(svg, s) for s in (256, 128, 64, 48, 32, 24, 16)]
    ico[0].save(os.path.join(HERE, "icon.ico"), sizes=[im.size for im in ico], append_images=ico[1:])
    ico[0].save(os.path.join(HERE, "icon-256.png"), optimize=True)


if __name__ == "__main__":
    main()
