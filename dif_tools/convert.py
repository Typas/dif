"""General image -> DIF conversion.

Reads an image with Pillow, picks ``indexed`` or ``grayscale`` mode, synthesizes
the dark theme with the chosen strategy, and encodes via the Rust ``dif`` module.
The *source* theme is always stored as the lossless identity.
"""

from __future__ import annotations

from pathlib import Path
from typing import cast

import dif
import numpy as np
from PIL import Image as PILImage

from .themes import STRATEGIES

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
        return rgba[..., 0].astype(np.uint8), True, 8
    return rgba, False, 8


# Rendered-PNG cache for ``.drawio`` inputs (gitignored; never /tmp).
_DRAWIO_PNG_CACHE = Path(__file__).resolve().parent.parent / "out" / "drawio-png"


def resolve_raster(path: str | Path) -> Path:
    """Map any input to a raster path PIL can open.

    A ``.drawio`` input is rendered to a PNG under ``out/drawio-png/`` (keeping
    ``testdata/`` clean); every other path passes through unchanged. Shared by
    the DIF converter and the format comparison so both see the same pixels.
    """
    path = Path(path)
    if path.suffix.lower() == ".drawio":
        from .drawio import render_drawio_to_png

        png = _DRAWIO_PNG_CACHE / (path.stem + ".png")
        # Reuse a cached render unless the source is newer (rendering shells out
        # to the drawio container/desktop — tens of seconds; benches re-resolve
        # the same diagram repeatedly).
        if png.exists() and png.stat().st_mtime >= path.stat().st_mtime:
            return png
        return Path(render_drawio_to_png(path, png))
    return path


def image_to_dif_image(path: str | Path, strategy: str = "arithmetic") -> "dif.Image":
    """Build a :class:`dif.Image` from an image (or ``.drawio``) file.

    A ``.drawio`` input is first rendered to a PNG under ``out/drawio-png/``
    (keeping ``testdata/`` clean) and then loaded like any raster image.
    """
    path = resolve_raster(path)
    arr, is_gray, depth_bits = load_image(path)
    return dif_image_from_array(arr, is_gray, depth_bits, strategy)


def dif_image_from_array(
    arr: np.ndarray, is_gray: bool, depth_bits: int, strategy: str = "arithmetic"
) -> "dif.Image":
    """Build a :class:`dif.Image` from an already-loaded raster array.

    Splits the in-memory build (palette/index + dark-theme synthesis) from disk
    I/O so callers that already hold the pixels — e.g. the format benchmark —
    can time *raw bitmap -> file* without re-reading the source. ``strategy``
    ``"keep"`` stores a single (light) theme; any other adds the dark theme.
    """
    if strategy not in STRATEGIES:
        raise ValueError(f"strategy must be one of {STRATEGIES}, got {strategy!r}")

    # Hand the raw bitmap to Rust and build the palette/index (color) or the
    # sample frame (grayscale) natively — like `png_encode(arr)` — instead of
    # running `np.unique`/`.tolist()` over millions of pixels and marshalling a
    # per-pixel list across PyO3 (that pixel work was ~99% of DIF encode time).
    if is_gray:
        h, w = arr.shape
        gray = (
            np.ascontiguousarray(arr, dtype=np.uint8)
            if depth_bits == 8
            else np.ascontiguousarray(arr, dtype="<u2")  # explicit little-endian
        )
        img = dif.Image.grayscale_from_samples(w, h, depth_bits, gray.tobytes())
    else:
        h, w = arr.shape[:2]
        rgba = np.ascontiguousarray(arr[..., :4], dtype=np.uint8).tobytes()
        img = dif.Image.indexed_from_rgba8(w, h, depth_bits, rgba)

    # Synthesize the dark theme natively: the derivation (OKLab palette / tone
    # LUT) runs in Rust off the small light theme, so no palette/LUT crosses the
    # FFI boundary. `"keep"` leaves the image single-theme.
    if strategy != "keep":
        # `strategy` is a runtime str (argparse-validated); narrow to the alias.
        img.add_dark_theme(cast("dif.Strategy", strategy))
    return img


def convert_file(
    input_path: str | Path,
    output_path: str | Path | None = None,
    strategy: str = "arithmetic",
    codec: str = "zstd-3",
    raw: bool = False,
) -> bytes:
    """Convert an image to ``.dif`` (or ``.difr`` if ``raw``); returns the bytes.

    ``codec`` is one of the study's variant strings (e.g. ``"zstd-3"``,
    ``"brotli-11"``, ``"lz4-fast1"``); see :data:`dif.CodecName`. A ``.drawio``
    input is rendered to PNG first (handled by :func:`image_to_dif_image`).
    """
    img = image_to_dif_image(input_path, strategy=strategy)
    # `codec` arrives as a runtime str (CLI/argparse); narrow to the typed alias.
    data = img.to_difr() if raw else img.to_dif(cast("dif.CodecName", codec))
    if output_path is not None:
        Path(output_path).write_bytes(data)
    return data
