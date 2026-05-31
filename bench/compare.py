"""Compare DIF against PNG / lossless JXL / WebP / AVIF / GIF.

Sizes, encode/decode speeds, and a losslessness check via ``imagecodecs`` (and
Pillow for GIF). Each format is guarded: a missing codec or a non-lossless
result is annotated rather than crashing the run. GIF and any format that fails
the lossless check are marked ``LOSSY`` so the size comparison stays honest.
"""

from __future__ import annotations

import io
import os
import statistics
from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import cast

import dif
import imagecodecs
import numpy as np
from PIL import Image as PILImage

from dif_tools import dif_image_from_array, load_image, resolve_raster

from .metric import speed

# The study's codec variants (plan.md). `DIF_BASELINE` names the shipped default
# codec; `REL_REF` is the `rel` reference column — every format's size is reported
# relative to it (PNG, the universal lossless baseline). `zstd-22` is zstd's max
# (ultra) level: slow/CPU-bound, the zstd analogue to brotli-11 for the `-mt` probe.
DIF_CODECS: tuple[str, ...] = (
    "zstd-3",
    "zstd-10",
    "zstd-22",
    "brotli-5",
    "brotli-11",
    "libdeflate-6",
    "lz4-fast1",
    "lzav-1",
)
DIF_BASELINE = "zstd-3"
REL_REF = "png"  # `rel` column reference format


@dataclass
class FormatResult:
    name: str
    size: int
    enc_mbps: float
    dec_mbps: float
    available: bool
    lossless: bool = True
    note: str = ""


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
    numthreads: int = 1,
) -> list[FormatResult]:
    # Render `.drawio` to PNG once; every format encoder then sees the same
    # raster (PIL/imagecodecs can't open the drawio XML directly).
    raster = resolve_raster(path)
    # `load_image` returns the natural dtype: uint8 for 8-bit (gray or RGBA),
    # uint16 for 16-bit grayscale — exactly what the encoders below expect.
    arr, is_gray, depth = load_image(raster)
    rgb = _as_rgb(arr, is_gray)
    nbytes = arr.nbytes
    rows: list[FormatResult] = []
    ref_size: int | None = None  # running reference for the live `rel` column

    if stream:
        print(_HEAD)
        print(_SEP)

    def emit(name: str, enc, dec, expected):
        """Measure one format, then print its row live (mirrors bench_image)."""
        nonlocal ref_size
        r = _measure(name, enc, dec, expected, nbytes, repeats)
        # PNG sets the running `rel` reference; the final table re-derives it via
        # _ref_size. dif rows stream before png, so their live rel stays blank
        # until png lands — the re-derived final/TSV/report tables are correct.
        if ref_size is None and r.available and name == REL_REF:
            ref_size = r.size
        rows.append(r)
        if stream:
            print(_row_line(r, ref_size))

    # DIF encode is timed *raw bitmap -> file*: the palette/index build AND the
    # dark-theme synthesis run inside the closure (parity with png_encode(arr),
    # which encodes the raw array). `arr` is already in memory, so no file I/O
    # is timed. `decode` renders one theme back to pixels (file -> bitmap).
    def dif_enc(strategy: str, codec: str, workers: int = 0):
        return lambda: dif_image_from_array(arr, is_gray, depth, strategy).to_dif(
            cast("dif.CodecName", codec), workers
        )

    def dif_dec(b: bytes):
        return dif.Image.from_dif(b).render("light", 0)[2]

    # The `rel` reference row (REL_REF picks it by name) is emitted first so every
    # row below has a live ratio and the table is topped by the baseline. png's
    # name and encoder are one unit; REL_REF only selects which row rel divides by.
    # FIXME: "emit ref first" assumes REL_REF == "png". If REL_REF moves to another
    # row, this hardcoded-first emit no longer matches it — rows above the real ref
    # get a stale live ratio. Drive the first-emit off REL_REF, or assert they agree.
    emit("png", lambda: imagecodecs.png_encode(arr), imagecodecs.png_decode, arr)

    # Headline row: the shipped default (zstd-3) carrying *both* themes
    # (light + dark) — the real `.dif` product, not directly size-comparable to
    # the single-image formats below.
    emit(f"dif-{DIF_BASELINE}-2t", dif_enc("arithmetic", DIF_BASELINE), dif_dec, None)

    # Codec comparison: one theme each, apples-to-apples with the single-image
    # formats (png/gif/webp/jxl/avif). DIF losslessness is covered in tests.
    for codec in dif_codecs:
        emit(f"dif-{codec}", dif_enc("keep", codec), dif_dec, None)

    # Multithreaded encode (`-mt`): same standard container, decoded single-
    # thread, so it charts the enc-speed/size tradeoff of workers > 1. zstd
    # (nbWorkers) and brotli (compress_multi) carry native workers; zstd barely
    # splits at diagram sizes, but the slow brotli levels scale ~linearly.
    if numthreads > 1:
        for codec in dif_codecs:
            if codec.startswith(("zstd-", "brotli-")):
                emit(
                    f"dif-{codec}-mt",
                    dif_enc("keep", codec, numthreads),
                    dif_dec,
                    None,
                )

    # avif/jxl pinned to their library's *native default* effort knob so the
    # comparison is reproducible: libjxl effort=7, libavif speed=6. (imagecodecs
    # leaves avif speed unset -> aom runs at speed 0, ~0.3 MB/s; we pin the
    # documented default instead.) webp keeps its own default. (png is emitted
    # above as the rel reference.) numthreads defaults to 1 so enc MB/s is an
    # apples-to-apples 1-core measure (dif/png/gif are single-threaded) and a
    # future imagecodecs `None`->auto default can't skew it; --numthreads N lifts
    # the cap to probe jxl/avif scaling (webp lossless ignores it).
    nt = numthreads
    emit(
        "webp-ll",
        lambda: imagecodecs.webp_encode(rgb, lossless=True, numthreads=nt),
        imagecodecs.webp_decode,
        rgb,
    )
    emit(
        "jxl-ll",
        lambda: imagecodecs.jpegxl_encode(arr, lossless=True, effort=7, numthreads=nt),
        imagecodecs.jpegxl_decode,
        arr,
    )
    emit(
        "avif-ll",
        lambda: imagecodecs.avif_encode(
            rgb, level=100, pixelformat="yuv444", speed=6, numthreads=nt
        ),
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


def _ref_size(rows: list[FormatResult]) -> int | None:
    """The size every other format's ``rel`` is measured against: ``REL_REF``
    (PNG), else the first available row in this run."""
    size = next((r.size for r in rows if r.name == REL_REF and r.available), None)
    if size is None:
        size = next((r.size for r in rows if r.available), None)
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


def _row_line(r: FormatResult, ref_size: int | None) -> str:
    if not r.available:
        return (
            f"| {r.name:^{_FMT_W}} | {'n/a':^{_SIZE_W}} | {'':^{_SPD_W}} "
            f"| {'':^{_SPD_W}} | {'':^{_REL_W}} | {r.note:^{_NOTE_W}} |"
        )
    rel = f"x{r.size / ref_size:.2f}" if ref_size else ""
    tag = "" if r.lossless else "LOSSY"
    return (
        f"| {r.name:^{_FMT_W}} | {r.size:>{_SIZE_W}} | {r.enc_mbps:>{_SPD_W}.1f} "
        f"| {r.dec_mbps:>{_SPD_W}.1f} | {rel:>{_REL_W}} | {tag:^{_NOTE_W}} |"
    )


def format_table(path: str | Path, rows: list[FormatResult]) -> str:
    lines = [_HEAD, _SEP]
    ref_size = _ref_size(rows)
    lines.extend(_row_line(r, ref_size) for r in rows)
    return "\n".join(lines)


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
    ref_size = _ref_size(rows)
    for r in rows:
        rel = f"{r.size / ref_size:.4f}" if (ref_size and r.available) else ""
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


# --- Aggregate stats (mirrors bench/runner.py's DirStat / subdir_stats) ------
# One row per image is detail (console + TSV); the report aggregates every
# format over all images beneath each directory, recursively.

# (path, rows) for one image — the unit aggregation buckets over.
ImageRows = tuple[str, list[FormatResult]]


@dataclass
class FormatStat:
    """One format aggregated over every image under a directory."""

    name: str
    n: int
    size_mean: float
    enc_mbps: float
    dec_mbps: float
    rel_mean: float
    lossless: bool
    note: str = ""


def _aggregate(reports: Sequence[ImageRows]) -> list[FormatStat]:
    by_fmt: dict[str, list[tuple[FormatResult, float | None]]] = defaultdict(list)
    for _path, rows in reports:
        ref_size = _ref_size(rows)
        for r in rows:
            if r.available:
                by_fmt[r.name].append((r, ref_size))
    stats: list[FormatStat] = []
    for name, items in by_fmt.items():
        rs = [r for r, _ in items]
        rels = [r.size / d for r, d in items if d]
        stats.append(
            FormatStat(
                name,
                len(rs),
                statistics.mean(r.size for r in rs),
                statistics.mean(r.enc_mbps for r in rs),
                statistics.mean(r.dec_mbps for r in rs),
                statistics.mean(rels) if rels else float("nan"),
                all(r.lossless for r in rs),
                rs[-1].note,
            )
        )
    # rel reference (REL_REF) first, then alphabetical by name (codecs cluster).
    stats.sort(key=lambda s: (s.name != REL_REF, s.name))
    return stats


def subdir_stats(reports: Sequence[ImageRows]) -> list[tuple[str, list[FormatStat]]]:
    """Aggregate per directory, recursively: every ancestor dir (down to the
    common root of the inputs) gets a stat block over all images beneath it.
    Returned outermost-first. Same shape as ``runner.subdir_stats``."""
    if not reports:
        return []
    paths = [Path(p).resolve() for p, _ in reports]
    root = Path(os.path.commonpath([str(p.parent) for p in paths]))
    buckets: dict[Path, list[ImageRows]] = defaultdict(list)
    for rep, p in zip(reports, paths):
        d = p.parent
        while True:
            buckets[d].append(rep)
            if d == root:
                break
            d = d.parent
    base = root.parent  # root shows by name, children as root/sub
    out: list[tuple[str, list[FormatStat]]] = []
    for d in sorted(buckets, key=lambda p: (len(p.parts), str(p))):
        out.append((os.path.relpath(d, base), _aggregate(buckets[d])))
    return out


def format_stats_table(stats: list[FormatStat]) -> str:
    """Aggregate block as a GitHub-flavored markdown table."""
    rows = [
        "| format | n | size | enc MB/s | dec MB/s | rel | lossless | note |",
        "|---|--:|--:|--:|--:|--:|:--:|---|",
    ]
    for s in stats:
        rel = "" if s.rel_mean != s.rel_mean else f"x{s.rel_mean:.2f}"  # nan guard
        ll = "yes" if s.lossless else "LOSSY"
        rows.append(
            f"| {s.name} | {s.n} | {s.size_mean:.0f} | {s.enc_mbps:.1f} "
            f"| {s.dec_mbps:.1f} | {rel} | {ll} | {s.note} |"
        )
    return "\n".join(rows)
