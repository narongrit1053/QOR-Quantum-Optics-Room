"""Renders the LabSetup app icon (crates/lab-ui/ui/assets/app-icon.svg) to PNG and .ico.

Geometry mirrors the SVG in its 100-unit viewBox; change both together.
Usage: python scripts/dev/make_icon.py   (needs Pillow)
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
ASSETS = ROOT / "crates/lab-ui/ui/assets"
BLUE, DARK, WHITE = "#0A84FF", "#0B0C0E", "#FFFFFF"


def render(size: int) -> Image.Image:
    ss = 4  # supersample, then downscale for anti-aliasing
    s = size * ss / 100
    img = Image.new("RGBA", (size * ss, size * ss), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    u = lambda *v: [round(x * s) for x in v]
    d.rounded_rectangle(u(0, 0, 100, 100), radius=round(22 * s), fill=BLUE)
    d.rounded_rectangle(u(26, 12, 44, 23), radius=round(3 * s), fill=DARK)
    w = 7 * s
    # Beam: square-capped polyline 35,23 -> 35,68 -> 100,68, drawn as two rectangles.
    d.rectangle([round(35 * s - w / 2), round(23 * s), round(35 * s + w / 2), round(68 * s + w / 2)], fill=WHITE)
    d.rectangle([round(35 * s - w / 2), round(68 * s - w / 2), size * ss, round(68 * s + w / 2)], fill=WHITE)
    # Mirror: round-capped 45° line.
    d.line(u(27, 60, 43, 76), fill=DARK, width=round(w))
    for cx, cy in ((27, 60), (43, 76)):
        d.ellipse([round(cx * s - w / 2), round(cy * s - w / 2), round(cx * s + w / 2), round(cy * s + w / 2)], fill=DARK)
    # Re-clip to the rounded tile so the beam never pokes past the corner radius.
    mask = Image.new("L", img.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(u(0, 0, 100, 100), radius=round(22 * s), fill=255)
    img.putalpha(Image.composite(img.getchannel("A"), mask, mask))
    return img.resize((size, size), Image.LANCZOS)


if __name__ == "__main__":
    render(256).save(ASSETS / "app-icon.png")
    big = render(256)
    big.save(ASSETS / "app-icon.ico", sizes=[(n, n) for n in (16, 20, 24, 32, 40, 48, 64, 128, 256)],
             append_images=[render(n) for n in (16, 20, 24, 32, 40, 48, 64, 128)])
    print("wrote", ASSETS / "app-icon.png", "and app-icon.ico")
