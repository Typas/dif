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

from dif_tools import dif_image_from_array, load_image, resolve_raster

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


def _load(path: str | Path) -> tuple[np.ndarray, bool, int]:
    arr, is_gray, depth = load_image(path)
    arr = arr.astype(np.uint8) if depth == 8 else arr.astype(np.uint16)
    return arr, is_gray, depth


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
    stream: bool = False,
) -> list[FormatResult]:
    # Render `.drawio` to PNG once; every format encoder then sees the same
    # raster (PIL/imagecodecs can't open the drawio XML directly).
    raster = resolve_raster(path)
    arr, is_gray, depth = _load(raster)
    rgb = _as_rgb(arr, is_gray)
    nbytes = arr.nbytes
    rows: list[FormatResult] = []
    dif_size: int | None = None  # running baseline for the live `rel` column

    if stream:
        print(_HEAD)
        print(_SEP)

    def emit(name: str, enc, dec, expected):
        """Measure one format, then print its row live (mirrors bench_image)."""
        nonlocal dif_size
        r = _measure(name, enc, dec, expected, nbytes, repeats)
        # The single-theme baseline variant sets the running `rel` reference;
        # the final table re-derives it via _baseline_size (prefers zstd-3).
        if dif_size is None and r.available and name == f"dif-{DIF_BASELINE}":
            dif_size = r.size
        rows.append(r)
        if stream:
            print(_row_line(r, dif_size))

    # DIF encode is timed *raw bitmap -> file*: the palette/index build AND the
    # dark-theme synthesis run inside the closure (parity with png_encode(arr),
    # which encodes the raw array). `arr` is already in memory, so no file I/O
    # is timed. `decode` renders one theme back to pixels (file -> bitmap).
    def dif_enc(strategy: str, codec: str):
        return lambda: dif_image_from_array(arr, is_gray, depth, strategy).to_dif(
            cast("dif.CodecName", codec)
        )

    def dif_dec(b: bytes):
        return dif.Image.from_dif(b).render("light", 0)[2]

    # Headline row: the shipped default (zstd-3) carrying *both* themes
    # (light + dark) — the real `.dif` product, not directly size-comparable to
    # the single-image formats below.
    emit(f"dif-{DIF_BASELINE}-2t", dif_enc("arithmetic", DIF_BASELINE), dif_dec, None)

    # Codec comparison: one theme each, apples-to-apples with the single-image
    # formats (png/gif/webp/jxl/avif). DIF losslessness is covered in tests.
    for codec in dif_codecs:
        emit(f"dif-{codec}", dif_enc("keep", codec), dif_dec, None)

    # avif/jxl pinned to their library's *native default* effort knob so the
    # comparison is reproducible: libjxl effort=7, libavif speed=6. (imagecodecs
    # leaves avif speed unset -> aom runs at speed 0, ~0.3 MB/s; we pin the
    # documented default instead.) webp keeps its own default.
    emit("png", lambda: imagecodecs.png_encode(arr), imagecodecs.png_decode, arr)
    emit(
        "webp-ll",
        lambda: imagecodecs.webp_encode(rgb, lossless=True),
        imagecodecs.webp_decode,
        rgb,
    )
    emit(
        "jxl-ll",
        lambda: imagecodecs.jpegxl_encode(arr, lossless=True, effort=7),
        imagecodecs.jpegxl_decode,
        arr,
    )
    emit(
        "avif-ll",
        lambda: imagecodecs.avif_encode(rgb, level=100, pixelformat="yuv444", speed=6),
        imagecodecs.avif_decode,
        rgb,
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

    emit("gif", gif_enc, gif_dec, arr if is_gray else rgb)
    return rows


def _baseline_size(rows: list[FormatResult]) -> int | None:
    """The DIF size every other format's ``rel`` is measured against: the
    baseline variant, else the first available ``dif-*`` row in this run."""
    baseline = f"dif-{DIF_BASELINE}"
    size = next((r.size for r in rows if r.name == baseline and r.available), None)
    if size is None:
        size = next(
            (r.size for r in rows if r.name.startswith("dif-") and r.available), None
        )
    return size


# Fixed-width pipe-table layout matching bench/runner.py's `bench_image` console
# output. Shared by the live stream and the final table so the two are identical.
_FMT_W = 16
_SIZE_W = 10
_SPD_W = 12
_REL_W = 6
_NOTE_W = 6
_HEAD = (
    f"| {'format':^{_FMT_W}} | {'size':^{_SIZE_W}} | {'enc MB/s':^{_SPD_W}} "
    f"| {'dec MB/s':^{_SPD_W}} | {'rel':^{_REL_W}} | {'note':^{_NOTE_W}} |"
)
_SEP = (
    f"|{'-' * (_FMT_W + 2)}|{'-' * (_SIZE_W + 2)}|{'-' * (_SPD_W + 2)}"
    f"|{'-' * (_SPD_W + 2)}|{'-' * (_REL_W + 2)}|{'-' * (_NOTE_W + 2)}|"
)


def _row_line(r: FormatResult, dif_size: int | None) -> str:
    if not r.available:
        return (
            f"| {r.name:^{_FMT_W}} | {'n/a':^{_SIZE_W}} | {'':^{_SPD_W}} "
            f"| {'':^{_SPD_W}} | {'':^{_REL_W}} | {r.note:^{_NOTE_W}} |"
        )
    rel = f"x{r.size / dif_size:.2f}" if dif_size else ""
    tag = "" if r.lossless else "LOSSY"
    return (
        f"| {r.name:^{_FMT_W}} | {r.size:>{_SIZE_W}} | {r.enc_mbps:>{_SPD_W}.1f} "
        f"| {r.dec_mbps:>{_SPD_W}.1f} | {rel:>{_REL_W}} | {tag:^{_NOTE_W}} |"
    )


def format_table(path: str | Path, rows: list[FormatResult]) -> str:
    lines = [_HEAD, _SEP]
    dif_size = _baseline_size(rows)
    lines.extend(_row_line(r, dif_size) for r in rows)
    return "\n".join(lines)


def markdown_table(path: str | Path, rows: list[FormatResult]) -> str:
    """The same comparison as a GitHub-flavored markdown table (for reports)."""
    dif_size = _baseline_size(rows)
    out = [
        f"### {Path(path).name}",
        "",
        "| format | size | enc MB/s | dec MB/s | rel | note |",
        "|---|--:|--:|--:|--:|---|",
    ]
    for r in rows:
        if not r.available:
            out.append(f"| {r.name} | n/a |  |  |  | {r.note} |")
            continue
        rel = f"x{r.size / dif_size:.2f}" if dif_size else ""
        tag = "" if r.lossless else "LOSSY"
        out.append(
            f"| {r.name} | {r.size} | {r.enc_mbps:.1f} | {r.dec_mbps:.1f} "
            f"| {rel} | {tag} |"
        )
    return "\n".join(out)


TSV_HEADER = (
    "image",
    "format",
    "size",
    "enc_mbps",
    "dec_mbps",
    "rel",
    "lossless",
    "available",
    "note",
)


def iter_rows(path: str | Path, rows: list[FormatResult]):
    """One flat row per (image, format) for CSV/TSV export."""
    name = Path(path).name
    dif_size = _baseline_size(rows)
    for r in rows:
        rel = f"{r.size / dif_size:.4f}" if (dif_size and r.available) else ""
        yield (
            name,
            r.name,
            r.size if r.available else "",
            f"{r.enc_mbps:.2f}" if r.available else "",
            f"{r.dec_mbps:.2f}" if r.available else "",
            rel,
            int(r.lossless),
            int(r.available),
            r.note,
        )
