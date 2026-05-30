"""Compare DIF against PNG / lossless JXL / WebP / AVIF / GIF.

Sizes, encode/decode speeds, and a losslessness check via ``imagecodecs`` (and
Pillow for GIF). Each format is guarded: a missing codec or a non-lossless
result is annotated rather than crashing the run. GIF and any format that fails
the lossless check are marked ``LOSSY`` so the size comparison stays honest.
"""

from __future__ import annotations

import io
from dataclasses import dataclass
from pathlib import Path
from typing import cast

import dif
import imagecodecs
import numpy as np
from PIL import Image as PILImage

from dif_tools import image_to_dif_image, load_image

from .metric import speed

# The study's 7 codec variants (plan.md). `DIF_BASELINE` is the `rel` reference
# column — every other format's size is reported relative to it.
DIF_CODECS: tuple[str, ...] = (
    "zstd-3",
    "zstd-10",
    "brotli-5",
    "brotli-11",
    "libdeflate-6",
    "lz4-fast1",
    "lzav-1",
)
DIF_BASELINE = "zstd-3"


@dataclass
class FormatResult:
    name: str
    size: int
    enc_mbps: float
    dec_mbps: float
    available: bool
    lossless: bool = True
    note: str = ""


def _load(path: str | Path) -> tuple[np.ndarray, bool]:
    arr, is_gray, depth = load_image(path)
    arr = arr.astype(np.uint8) if depth == 8 else arr.astype(np.uint16)
    return arr, is_gray


def _as_rgb(arr: np.ndarray, is_gray: bool) -> np.ndarray:
    """A contiguous 3-channel array for codecs that reject single-channel input."""
    rgb = np.repeat(arr[..., None], 3, axis=2) if is_gray else arr[..., :3]
    return np.ascontiguousarray(rgb)


def _equal(decoded: np.ndarray, expected: np.ndarray) -> bool:
    d = np.asarray(decoded)
    if d.shape != expected.shape:
        if d.ndim == 3 and expected.ndim == 2 and d.shape[2] >= 1:
            d = d[..., 0]
        elif d.ndim == expected.ndim and d.shape[:2] == expected.shape[:2]:
            d = d[..., : expected.shape[2]] if expected.ndim == 3 else d
    return d.shape == expected.shape and bool(np.array_equal(d, expected))


def _measure(
    name: str, enc, dec, expected: np.ndarray | None, nbytes: int, repeats: int
) -> FormatResult:
    try:
        blob = enc()
        decoded = dec(blob)
        lossless = True if expected is None else _equal(decoded, expected)
        e, _ = speed(enc, nbytes, repeats)
        d, _ = speed(lambda: dec(blob), nbytes, repeats)
        return FormatResult(name, len(blob), e / 1e6, d / 1e6, True, lossless)
    except Exception as exc:  # noqa: BLE001
        return FormatResult(name, 0, 0, 0, False, note=type(exc).__name__)


def compare_image(
    path: str | Path,
    repeats: int = 3,
    dif_codecs: tuple[str, ...] | list[str] = DIF_CODECS,
) -> list[FormatResult]:
    arr, is_gray = _load(path)
    rgb = _as_rgb(arr, is_gray)
    nbytes = arr.nbytes
    rows: list[FormatResult] = []

    img = image_to_dif_image(path, "arithmetic")
    for codec in dif_codecs:
        rows.append(
            _measure(
                f"dif-{codec}",
                # default arg binds `codec` per iteration (avoids late binding);
                # `codec` is a runtime str, narrow to the typed alias.
                lambda c=codec: img.to_dif(cast("dif.CodecName", c)),
                lambda b: dif.Image.from_dif(b).render("light", 0)[2],
                None,  # DIF losslessness is verified in tests/test_convert.py
                nbytes,
                repeats,
            )
        )

    rows.append(
        _measure(
            "png",
            lambda: imagecodecs.png_encode(arr),
            imagecodecs.png_decode,
            arr,
            nbytes,
            repeats,
        )
    )
    rows.append(
        _measure(
            "webp-ll",
            lambda: imagecodecs.webp_encode(rgb, lossless=True),
            imagecodecs.webp_decode,
            rgb,
            nbytes,
            repeats,
        )
    )
    rows.append(
        _measure(
            "jxl-ll",
            lambda: imagecodecs.jpegxl_encode(arr, lossless=True),
            imagecodecs.jpegxl_decode,
            arr,
            nbytes,
            repeats,
        )
    )
    rows.append(
        _measure(
            "avif-ll",
            lambda: imagecodecs.avif_encode(rgb, level=100, pixelformat="yuv444"),
            imagecodecs.avif_decode,
            rgb,
            nbytes,
            repeats,
        )
    )

    # GIF via Pillow (palette; lossless only for <=256 colors).
    pil = PILImage.fromarray(arr if is_gray else rgb)

    def gif_enc() -> bytes:
        buf = io.BytesIO()
        pil.quantize(colors=256).save(buf, format="GIF")
        return buf.getvalue()

    def gif_dec(b: bytes) -> np.ndarray:
        out = PILImage.open(io.BytesIO(b))
        return np.asarray(out.convert("L") if is_gray else out.convert("RGB"))

    rows.append(
        _measure("gif", gif_enc, gif_dec, arr if is_gray else rgb, nbytes, repeats)
    )
    return rows


def format_table(path: str | Path, rows: list[FormatResult]) -> str:
    head = f"{'format':<16}{'size':>10}{'enc MB/s':>10}{'dec MB/s':>10}{'rel':>7}  note"
    lines = [f"# {Path(path).name}", head, "-" * len(head)]
    # `rel` is measured against the baseline DIF variant; fall back to the first
    # available dif-* row if the baseline wasn't part of this run.
    baseline = f"dif-{DIF_BASELINE}"
    dif_size = next((r.size for r in rows if r.name == baseline and r.available), None)
    if dif_size is None:
        dif_size = next(
            (r.size for r in rows if r.name.startswith("dif-") and r.available), None
        )
    for r in rows:
        if not r.available:
            lines.append(f"{r.name:<16}{'n/a':>10}{'':>10}{'':>10}{'':>7}  {r.note}")
            continue
        rel = f"x{r.size / dif_size:.2f}" if dif_size else ""
        tag = "" if r.lossless else "LOSSY"
        lines.append(
            f"{r.name:<16}{r.size:>10}{r.enc_mbps:>10.1f}{r.dec_mbps:>10.1f}{rel:>7}  {tag}"
        )
    return "\n".join(lines)
