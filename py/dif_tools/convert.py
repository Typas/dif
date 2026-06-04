"""General image -> DIF conversion.

Reads an image with Pillow as RGBA8, builds the palette/index natively,
synthesizes the dark theme with the chosen strategy, and encodes via the Rust
``dif`` module. The *source* (light) theme is always stored as the lossless
identity. v3 is indexed-only — there is no grayscale mode.
"""

from __future__ import annotations

from pathlib import Path
from typing import cast

import dif
import numpy as np
from PIL import Image as PILImage

from .themes import STRATEGIES


def load_image(path: str | Path) -> np.ndarray:
    """Return an ``(H, W, 4)`` RGBA8 array for any image (or rendered ``.drawio``).

    16-bit and grayscale inputs are flattened to RGBA8 by Pillow, since v3 stores
    every image as an indexed RGBA palette.
    """
    im = PILImage.open(path)
    return np.asarray(im.convert("RGBA"))


# Rendered-PNG cache for ``.drawio`` inputs (gitignored; never /tmp).
_DRAWIO_PNG_CACHE = Path(__file__).resolve().parent.parent.parent / "out" / "drawio-png"


def resolve_raster(path: str | Path) -> Path:
    """Map any input to a raster path PIL can open.

    A ``.drawio`` input is rendered to a PNG under ``out/drawio-png/`` (keeping
    ``data/testdata/`` clean); every other path passes through unchanged. Shared by
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


def _index_width_arg(index_width: str) -> int | None:
    """Map the ``"auto"``/``"8"``/``"16"`` CLI string to the binding's
    ``index_width`` (``None`` for auto, else the bit count)."""
    if index_width == "auto":
        return None
    if index_width in ("8", "16"):
        return int(index_width)
    raise ValueError(f"index_width must be 'auto', '8', or '16', got {index_width!r}")


def image_to_dif_image(
    path: str | Path, strategy: str = "arithmetic", index_width: str = "auto"
) -> "dif.Image":
    """Build a :class:`dif.Image` from an image (or ``.drawio``) file.

    A ``.drawio`` input is first rendered to a PNG under ``out/drawio-png/``
    (keeping ``data/testdata/`` clean) and then loaded like any raster image.
    """
    arr = load_image(resolve_raster(path))
    return dif_image_from_array(arr, strategy, index_width)


def dif_image_from_array(
    arr: np.ndarray, strategy: str = "arithmetic", index_width: str = "auto"
) -> "dif.Image":
    """Build a :class:`dif.Image` from an already-loaded ``(H, W, 4)`` RGBA8 array.

    Splits the in-memory build (palette/index + dark-theme synthesis) from disk
    I/O so callers that already hold the pixels — e.g. the format benchmark —
    can time *raw bitmap -> file* without re-reading the source. ``strategy``
    ``"keep"`` stores a single (light) theme; any other adds the dark theme.
    ``index_width`` is ``"auto"`` (smallest fitting width, quantizing only above
    16-bit), ``"8"``, or ``"16"`` (force that width, quantizing down to fit).
    """
    if strategy not in STRATEGIES:
        raise ValueError(f"strategy must be one of {STRATEGIES}, got {strategy!r}")
    iw = _index_width_arg(index_width)

    # Hand the raw bitmap to Rust and build the palette/index natively — like
    # `png_encode(arr)` — instead of running `np.unique`/`.tolist()` over millions
    # of pixels and marshalling a per-pixel list across PyO3 (that pixel work was
    # ~99% of DIF encode time).
    h, w = arr.shape[:2]
    rgba = np.ascontiguousarray(arr[..., :4], dtype=np.uint8).tobytes()
    img = dif.Image.indexed_from_rgba8(w, h, rgba, iw)

    # Synthesize the dark theme natively: the OKLab palette derivation runs in
    # Rust off the small light palette, so no palette crosses the FFI boundary.
    # `"keep"` leaves the image single-theme.
    if strategy != "keep":
        # `strategy` is a runtime str (argparse-validated); narrow to the alias.
        img.add_dark_theme(cast("dif.Strategy", strategy))
    return img


def convert_file(
    input_path: str | Path,
    output_path: str | Path | None = None,
    strategy: str = "arithmetic",
    codec: str = "zstd-3",
    palette_codec: str = "store",
    frame_codec: str = "store",
    raw: bool = False,
    index_width: str = "auto",
) -> bytes:
    """Convert an image to ``.dif`` (or ``.difr`` if ``raw``); returns the bytes.

    ``codec`` is the outer whole-body codec; ``palette_codec``/``frame_codec``
    compress the palette and per-frame sections (default ``"store"`` for the
    random-access layout). Each is one of the study's variant strings (e.g.
    ``"zstd-3"``, ``"brotli-11"``, ``"lz4-fast1"``); see :data:`dif.CodecName`.
    ``index_width`` is ``"auto"``/``"8"``/``"16"`` (see :func:`dif_image_from_array`).
    """
    img = image_to_dif_image(input_path, strategy=strategy, index_width=index_width)
    if raw:
        data = img.to_difr()
    else:
        data = img.to_dif(
            cast("dif.CodecName", codec),
            cast("dif.CodecName", palette_codec),
            cast("dif.CodecName", frame_codec),
        )
    if output_path is not None:
        Path(output_path).write_bytes(data)
    return data
