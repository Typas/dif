"""General image -> DIF conversion.

Reads an image with Pillow, picks ``indexed`` or ``grayscale`` mode, synthesizes
the dark theme with the chosen strategy, and encodes via the Rust ``dif`` module.
The *source* theme is always stored as the lossless identity.
"""

from __future__ import annotations

from pathlib import Path

import dif
import numpy as np
from PIL import Image as PILImage

from .themes import STRATEGIES, derive_lut, derive_palette, identity_lut

_GRAY_16_MODES = {"I", "I;16", "I;16B", "I;16L", "I;16N"}


def _looks_grayscale(rgba: np.ndarray) -> bool:
    rgb = rgba[..., :3]
    return bool(
        np.array_equal(rgb[..., 0], rgb[..., 1])
        and np.array_equal(rgb[..., 1], rgb[..., 2])
    )


def load_image(path: str | Path) -> tuple[np.ndarray, bool, int]:
    """Return ``(array, is_grayscale, depth_bits)``.

    Grayscale arrays are 2-D ``(H, W)``; color arrays are ``(H, W, 4)`` RGBA.
    """
    im = PILImage.open(path)
    if im.mode in _GRAY_16_MODES:
        arr = np.asarray(im).astype(np.uint16)
        return arr, True, 16
    rgba = np.asarray(im.convert("RGBA"))
    if _looks_grayscale(rgba) and bool(np.all(rgba[..., 3] == 255)):
        return rgba[..., 0].astype(np.uint16), True, 8
    return rgba, False, 8


# Rendered-PNG cache for ``.drawio`` inputs (gitignored; never /tmp).
_DRAWIO_PNG_CACHE = Path(__file__).resolve().parent.parent / "out" / "drawio-png"


def image_to_dif_image(path: str | Path, strategy: str = "arithmetic") -> "dif.Image":
    """Build a :class:`dif.Image` from an image (or ``.drawio``) file.

    A ``.drawio`` input is first rendered to a PNG under ``out/drawio-png/``
    (keeping ``testdata/`` clean) and then loaded like any raster image.
    """
    if strategy not in STRATEGIES:
        raise ValueError(f"strategy must be one of {STRATEGIES}, got {strategy!r}")
    path = Path(path)
    if path.suffix.lower() == ".drawio":
        from .drawio import render_drawio_to_png

        png = _DRAWIO_PNG_CACHE / (path.stem + ".png")
        path = Path(render_drawio_to_png(path, png))
    arr, is_gray, depth_bits = load_image(path)
    max_value = (1 << depth_bits) - 1

    if is_gray:
        h, w = arr.shape
        samples = arr.reshape(-1).astype(np.int64).tolist()
        themes = [(0, "light")]
        luts = [identity_lut(max_value)]
        if strategy != "keep":
            themes.append((1, "dark"))
            luts.append(derive_lut(strategy, max_value))
        return dif.Image.grayscale(w, h, depth_bits, themes, luts, [samples])

    h, w = arr.shape[:2]
    flat = arr.reshape(-1, 4)
    colors, inverse = np.unique(flat, axis=0, return_inverse=True)
    inverse = inverse.reshape(-1)
    themes = [(0, "light")]
    palettes = [_to_palette(colors)]
    if strategy != "keep":
        dark = derive_palette(colors.astype(np.int64), strategy, max_value)
        themes.append((1, "dark"))
        palettes.append(_to_palette(dark))
    frames = [inverse.astype(np.int64).tolist()]
    return dif.Image.indexed(w, h, depth_bits, themes, palettes, frames)


def _to_palette(colors: np.ndarray) -> list[tuple[int, int, int, int]]:
    return [(int(c[0]), int(c[1]), int(c[2]), int(c[3])) for c in colors]


def convert_file(
    input_path: str | Path,
    output_path: str | Path | None = None,
    strategy: str = "arithmetic",
    codec: str = "brotli",
    raw: bool = False,
) -> bytes:
    """Convert an image to ``.dif`` (or ``.difr`` if ``raw``); returns the bytes.

    A ``.drawio`` input is rendered to PNG first (handled by
    :func:`image_to_dif_image`).
    """
    img = image_to_dif_image(input_path, strategy=strategy)
    data = img.to_difr() if raw else img.to_dif(codec)
    if output_path is not None:
        Path(output_path).write_bytes(data)
    return data
