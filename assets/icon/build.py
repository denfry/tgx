"""Generate the tgx app icon, tray icons and README banner.

The mark is defined once, in a 256-unit grid, below. Each output size gets its own
stroke weights so the glyph stays legible at 16 px instead of being a scaled-down
256 px drawing. Run from anywhere:

    python assets/icon/build.py

Requires `resvg-py` and `Pillow`. Writes SVG sources, PNG previews and .ico files
next to this script; the outputs are committed, so the build does not need Python.
"""

from __future__ import annotations

import io
import math
from pathlib import Path

import resvg_py
from PIL import Image

OUT = Path(__file__).resolve().parent

# Tokens from docs/specs/2026-11-10-tgx-ui-design.md, section 2.1.
ACCENT = "#2f6f62"
ON_ACCENT = "#f7f6f3"  # `win` light; warmer than pure white on the teal plate
INK_LIGHT_BAR = "#1d1d1b"  # `text` light: glyph on a light taskbar
INK_DARK_BAR = "#e6e5e0"  # `text` dark: glyph on a dark taskbar
STATE = {  # (light taskbar, dark taskbar)
    "ok": ("#27704a", "#6cc28b"),
    "warn": ("#8a5a0c", "#e0a64a"),
    "bad": ("#b5402f", "#e07a68"),
}

# --- Geometry, 256-unit grid -------------------------------------------------
# Speech bubble: an ellipse broken at the lower left by a tail.
CX, CY, RX, RY = 128.0, 106.0, 76.0, 58.0
TAIL_FROM_DEG, TAIL_TO_DEG = 105.0, 140.0  # where the tail leaves the ellipse
TAIL_TIP = (70.0, 186.0)
# Route: hops under the bubble, ending up and to the right.
HOPS = [(128.0, 200.0), (168.0, 206.0), (202.0, 172.0)]


def _on_ellipse(deg: float) -> tuple[float, float]:
    t = math.radians(deg)
    return CX + RX * math.cos(t), CY + RY * math.sin(t)


def bubble_path() -> str:
    a = _on_ellipse(TAIL_FROM_DEG)
    b = _on_ellipse(TAIL_TO_DEG)
    # Long way round (large-arc=1), counter-clockwise on screen (sweep=0).
    return (
        f"M{a[0]:.2f} {a[1]:.2f}"
        f"A{RX} {RY} 0 1 0 {b[0]:.2f} {b[1]:.2f}"
        f"L{TAIL_TIP[0]} {TAIL_TIP[1]}Z"
    )


def glyph(color: str, px: int, *, last_hop: str | None = None, tray: bool = False) -> str:
    """The mark at a target pixel size. Weights are given in output pixels."""
    u = 256 / px  # units per output pixel
    # (bubble stroke, route stroke, hop radius) in output pixels.
    if px >= 128:
        w_bubble, w_route, r_hop = 14 / u, 6 / u, 10 / u
    elif px >= 40:
        w_bubble, w_route, r_hop = 2.75, 1.25, 1.9
    elif px >= 28:
        w_bubble, w_route, r_hop = 2.0, 1.25, 1.6
    else:
        w_bubble, w_route, r_hop = 1.5, 1.0, 1.6
    # Below 28 px the route turns into noise next to the tail: keep only the last hop.
    hops = HOPS if px >= 28 else HOPS[-1:]
    last_r = r_hop * (1.6 if tray else 1.0)
    pts = " ".join(f"{x},{y}" for x, y in hops)
    parts = [
        f'<path d="{bubble_path()}" fill="none" stroke="{color}" '
        f'stroke-width="{w_bubble * u:.2f}" stroke-linejoin="round"/>',
    ]
    if len(hops) > 1:
        parts.append(
            f'<polyline points="{pts}" fill="none" stroke="{color}" '
            f'stroke-width="{w_route * u:.2f}" stroke-linecap="round" stroke-linejoin="round"/>'
        )
    for x, y in hops[:-1]:
        parts.append(f'<circle cx="{x}" cy="{y}" r="{r_hop * u:.2f}" fill="{color}"/>')
    x, y = hops[-1]
    parts.append(f'<circle cx="{x}" cy="{y}" r="{last_r * u:.2f}" fill="{last_hop or color}"/>')
    return "".join(parts)


def svg(body: str, size: int = 256) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" '
        f'viewBox="0 0 256 256">{body}</svg>\n'
    )


def app_icon(px: int) -> str:
    inset = 0 if px <= 24 else 8  # full bleed where every pixel counts
    radius = 3.5 * 256 / px if px <= 24 else 52
    plate = (
        f'<rect x="{inset}" y="{inset}" width="{256 - 2 * inset}" height="{256 - 2 * inset}" '
        f'rx="{radius:.2f}" fill="{ACCENT}"/>'
    )
    return svg(plate + glyph(ON_ACCENT, px), px)


def tray_icon(px: int, ink: str, state_color: str | None) -> str:
    return svg(glyph(ink, px, last_hop=state_color, tray=True), px)


def render(svg_text: str, px: int) -> Image.Image:
    png = resvg_py.svg_to_bytes(svg_string=svg_text, width=px, height=px)
    return Image.open(io.BytesIO(bytes(png))).convert("RGBA")


def write_ico(path: Path, frames: dict[int, Image.Image]) -> None:
    sizes = sorted(frames)
    largest = frames[sizes[-1]]
    # Pillow takes the extra frames via `append_images` and keeps them unscaled
    # when their size is listed in `sizes`.
    largest.save(
        path,
        format="ICO",
        sizes=[(s, s) for s in sizes],
        append_images=[frames[s] for s in sizes[:-1]],
    )


def banner() -> str:
    w, h = 1280, 640
    icon = app_icon(256).replace('width="256" height="256"', 'x="150" y="200" width="240" height="240"')
    line_y, xs = 320, [520, 760, 1000]
    hops = "".join(f'<circle cx="{x}" cy="{line_y}" r="13" fill="{ACCENT}"/>' for x in xs)
    # Filled bubble at the end of the route, same tail as the mark.
    end = (
        f'<g transform="translate(1030 256) scale(0.62)">'
        f'<path d="{bubble_path()}" fill="{ACCENT}" stroke="{ACCENT}" stroke-width="6" '
        f'stroke-linejoin="round"/></g>'
    )
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">'
        f'<rect width="{w}" height="{h}" fill="{ON_ACCENT}"/>{icon}'
        f'<line x1="{xs[0]}" y1="{line_y}" x2="{xs[-1]}" y2="{line_y}" stroke="{ACCENT}" stroke-width="4"/>'
        f"{hops}{end}</svg>\n"
    )


def main() -> None:
    ico_sizes = [16, 20, 24, 32, 40, 48, 64, 256]
    tray_sizes = [16, 20, 24, 32]

    (OUT / "tgx.svg").write_text(app_icon(256), encoding="utf-8")
    (OUT / "tgx-16.svg").write_text(app_icon(16), encoding="utf-8")
    (OUT / "tgx-32.svg").write_text(app_icon(32), encoding="utf-8")
    app_frames = {s: render(app_icon(s), s) for s in ico_sizes}
    write_ico(OUT / "tgx.ico", app_frames)
    app_frames[256].save(OUT / "tgx-256.png")

    for bar, ink, idx in (("light", INK_LIGHT_BAR, 0), ("dark", INK_DARK_BAR, 1)):
        for state in ("idle", *STATE):
            color = STATE[state][idx] if state in STATE else None
            name = f"tray-{bar}-{state}"
            (OUT / f"{name}.svg").write_text(tray_icon(32, ink, color), encoding="utf-8")
            write_ico(OUT / f"{name}.ico", {s: render(tray_icon(s, ink, color), s) for s in tray_sizes})

    banner_svg = banner()
    (OUT / "banner.svg").write_text(banner_svg, encoding="utf-8")
    png = resvg_py.svg_to_bytes(svg_string=banner_svg)
    (OUT / "banner.png").write_bytes(bytes(png))


if __name__ == "__main__":
    main()
