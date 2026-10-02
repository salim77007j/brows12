#!/usr/bin/env python3
"""Compose side-by-side comparison images: Chrome (left) vs brows12 (right)."""
import pathlib
from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
REF = ROOT / "validation" / "run" / "reference"
FIN = ROOT / "validation" / "run" / "final"
OUT = ROOT / "screenshots" / "compare"

SITES = [
    "wikipedia_rust",
    "rust_lang_org",
    "github_home",
    "hacker_news",
    "example_com",
    "bing_search",
]

LABEL_H = 28


def compose(name):
    ref = REF / f"{name}.png"
    fin = FIN / f"{name}.png"
    if not ref.exists() or not fin.exists():
        print(f"  SKIP {name} (missing {'ref' if not ref.exists() else 'render'})")
        return
    a = Image.open(ref).convert("RGB")
    b = Image.open(fin).convert("RGB")
    h = max(a.height, b.height)
    w = a.width + b.width + 12
    canvas = Image.new("RGB", (w, h + LABEL_H), (24, 26, 27))
    d = ImageDraw.Draw(canvas)
    d.text((8, 7), f"{name} — LEFT: Chromium (ground truth) | RIGHT: brows12 v0.4.0", fill=(255, 255, 255))
    canvas.paste(a, (0, LABEL_H))
    canvas.paste(b, (a.width + 12, LABEL_H))
    out = OUT / f"{name}_compare.png"
    canvas.save(out)
    print(f"  {out.relative_to(ROOT)}")


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for s in SITES:
        compose(s)


if __name__ == "__main__":
    main()
