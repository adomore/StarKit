#!/usr/bin/env python3
"""Render README preview images from the generated golden fixtures.

Documentation tooling only — nothing here is part of the product, and nothing
downstream depends on it. It exists so the images under `docs/images/` are
regenerable rather than mystery binaries in git history.

Fixture images are linear 16-bit: displayed raw they are almost black, because
the interesting signal sits a few hundred ADU above a 300 ADU background while
the bright tail runs to 65535. So previews get an asinh display stretch — the
same thing an astrophotographer does in post. This is presentation only and has
no bearing on the photometry the fixtures encode.

Usage (after `cargo run --release -p starkit-fixtures -- gen --suite all --out fixtures/generated`):

    python tools/make_previews.py
"""

from __future__ import annotations

import json
import pathlib

import numpy as np
import tifffile
from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
GENERATED = ROOT / "fixtures" / "generated"
OUT = ROOT / "docs" / "images"

# Asinh softening. Higher = more aggressive lift of the faint end, so faint
# stars survive the downsample to preview size.
ASINH_A = 300.0
# Clip point for the bright end. Stars are sparse, so a high percentile keeps
# the background from being crushed while letting the bright tail saturate.
HI_PERCENTILE = 99.95


def stretch(img: np.ndarray) -> np.ndarray:
    """Linear 16-bit → display-referred float in [0, 1] via an asinh stretch."""
    x = img.astype(np.float64)
    bkg = float(np.median(x))
    hi = float(np.percentile(x, HI_PERCENTILE))
    if hi <= bkg:
        hi = bkg + 1.0
    x = np.clip((x - bkg) / (hi - bkg), 0.0, 1.0)
    return np.arcsinh(x * ASINH_A) / np.arcsinh(ASINH_A)


def to_image(arr: np.ndarray) -> Image.Image:
    return Image.fromarray((np.clip(arr, 0, 1) * 255).astype(np.uint8))


def load(suite: str) -> np.ndarray:
    path = GENERATED / suite / "image.tiff"
    if not path.exists():
        raise SystemExit(
            f"missing {path}\n"
            "Generate the fixtures first:\n"
            "  cargo run --release -p starkit-fixtures -- gen --suite all --out fixtures/generated"
        )
    return tifffile.imread(path)


def brightest_star(suite: str) -> tuple[float, float]:
    """Centre a crop on the suite's brightest star, from the truth catalog."""
    cat = json.loads((GENERATED / suite / "truth.json").read_text())
    star = max(cat["stars"], key=lambda s: s["flux"])
    return star["x"], star["y"]


def crop(suite: str, size: int, centre: tuple[float, float] | None = None) -> Image.Image:
    """A 100 %-scale crop — the only honest way to show PSF detail."""
    img = load(suite)
    h, w = img.shape[:2]
    cx, cy = centre if centre else (w / 2, h / 2)
    x0 = int(np.clip(cx - size / 2, 0, w - size))
    y0 = int(np.clip(cy - size / 2, 0, h - size))
    return to_image(stretch(img)[y0 : y0 + size, x0 : x0 + size])


def label(im: Image.Image, text: str) -> Image.Image:
    d = ImageDraw.Draw(im)
    # Shadow then text: legible over both black sky and a blown-out core.
    d.text((9, 9), text, fill=(0, 0, 0))
    d.text((8, 8), text, fill=(255, 255, 255))
    return im


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)

    # Hero: the nightscape frame beside its truth sky mask. This pair is the
    # whole point of INV-1 — every star operation is confined to the white area.
    night = to_image(stretch(load("nightscape-fg")))
    mask = Image.open(GENERATED / "nightscape-fg" / "sky_mask.png").convert("RGB")
    # Side by side, not stacked: a 3:2 frame stacked twice renders absurdly tall
    # in a README column.
    half = 560
    size = (half, int(night.height * (half / night.width)))
    night = label(night.resize(size, Image.LANCZOS), "nightscape-fg  6144x4096")
    mask = label(mask.resize(size, Image.LANCZOS), "truth sky mask")

    gap = 6
    hero = Image.new("RGB", (half * 2 + gap, size[1]), (20, 20, 24))
    hero.paste(night, (0, 0))
    hero.paste(mask, (half + gap, 0))
    hero.save(OUT / "nightscape.png", optimize=True)

    # Crops at 100 %: PSF, crowding, deblending, and saturation structure.
    tiles = [
        (crop("basic-5k", 360), "basic-5k"),
        (crop("dense-core", 360), "dense-core  25k stars"),
        (crop("pairs", 360), "pairs  0.5-2.0x FWHM"),
        (crop("saturated", 360, brightest_star("saturated")), "saturated  halo + bleed"),
    ]
    gap = 6
    strip = Image.new(
        "RGB", (360 * len(tiles) + gap * (len(tiles) - 1), 360), (20, 20, 24)
    )
    for i, (tile, text) in enumerate(tiles):
        strip.paste(label(tile.convert("RGB"), text), (i * (360 + gap), 0))
    strip.save(OUT / "suites.png", optimize=True)

    for p in (OUT / "nightscape.png", OUT / "suites.png"):
        print(f"wrote {p.relative_to(ROOT)}  ({p.stat().st_size / 1024:.0f} KB)")


if __name__ == "__main__":
    main()
