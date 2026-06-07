"""Compare DIF against PNG / lossless JXL / WebP / AVIF / GIF.

Sizes and encode/decode speeds via ``imagecodecs`` (and Pillow for GIF). Each
format is guarded: a missing codec is annotated rather than crashing the run.
Every format here is lossless by construction, so no loss column is reported; a
round-trip check still runs (``FormatResult.lossless``, asserted in tests) to
catch a codec that silently stops round-tripping. GIF is genuine lossless LZW ---
it only diverges when an image exceeds its 256-entry palette (width, not loss).
"""

from __future__ import annotations

import io
import os
import statistics
from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

import dif
import imagecodecs
import numpy as np
from PIL import Image as PILImage

from dif_tools import dif_image_from_array, load_image, resolve_raster

from .metric import compute_m, speed

# `DIF_TRIPLET` is the shipped encode config --- (outer, palette, frame): outer
# `store` keeps frames seekable for random-access / low-memory decode, palette
# `zstd-16`, frame `zstd-10`. It is the default `bench formats` DIF row; sweep any
# section at runtime with the lzbench-style `--dif-*codecs` flags (no static study
# set --- the flags build any sweep or matrix). `REL_REF` is the `rel` reference
# column --- every format's size is relative to it (PNG, the universal lossless
# baseline).
DIF_TRIPLET: tuple[str, str, str] = ("store", "zstd-16", "zstd-10")
REL_REF = "png"  # `rel` column reference format

# Compact codec abbreviations for the DIF row labels.
_FAMILY_ABBR = {
    "deflate": "PK",
    "libdeflate": "PK",
    "brotli": "br",
    "bsc": "bsc",
    "zstd": "zst",
    "lzav": "av",
}
# Bare-family aliases resolve to their study-default level for display.
_DEFAULT_LEVEL = {
    "deflate": "6",
    "libdeflate": "6",
    "brotli": "5",
    "bsc": "2",
    "zstd": "3",
    "lzav": "1",
}


def _abbr(variant: str) -> str:
    """Abbreviate a codec variant, e.g. zstd-3->zst3, lz4-fast1->4f1,
    lz4-hc9->4hc9, libdeflate-6->PK6, store->st."""
    if variant == "store":
        return "st"
    if variant.startswith("lz4"):
        rest = variant[3:].lstrip("-")
        if rest.startswith("fast"):
            return "4f" + rest[4:]
        if rest.startswith("hc"):
            return "4hc" + rest[2:]
        return "4f1"  # bare "lz4"
    fam, _, lvl = variant.partition("-")
    base = _FAMILY_ABBR.get(fam)
    if base is None:
        return variant  # unknown --- show verbatim (e.g. a typo'd codec)
    return base + (lvl or _DEFAULT_LEVEL[fam])


def _dif_label(outer: str, palette: str, frame: str, index_bits: int) -> str:
    """`dif-<outer>-<Nb>` when palette/frame inherit the outer codec, else
    `dif-<outer>-<palette>-<frame>-<Nb>`. The `-<Nb>` suffix (`-8b`/`-16b`) is the
    *resolved* index width, so the row shows which profile the build landed on."""
    if palette == outer and frame == outer:
        return f"dif-{_abbr(outer)}-{index_bits}b"
    return f"dif-{_abbr(outer)}-{_abbr(palette)}-{_abbr(frame)}-{index_bits}b"


@dataclass
class FormatResult:
    name: str
    size: int
    enc_mbps: float
    dec_mbps: float
    available: bool
    lossless: bool = True
    note: str = ""
    m: float = float("nan")  # M score relative to store baseline (dif_only mode)


def _as_rgb(arr: np.ndarray) -> np.ndarray:
    """A contiguous 3-channel (RGB) view of an RGBA array."""
    return np.ascontiguousarray(arr[..., :3])


def _equal(decoded: np.ndarray, expected: np.ndarray) -> bool:
    d = np.asarray(decoded)
    if d.shape != expected.shape:
        if d.ndim == 3 and expected.ndim == 2 and d.shape[2] >= 1:
            d = d[..., 0]
        elif d.ndim == expected.ndim and d.shape[:2] == expected.shape[:2]:
            d = d[..., : expected.shape[2]] if expected.ndim == 3 else d
    return d.shape == expected.shape and bool(np.array_equal(d, expected))


def _measure(
    name: str,
    enc,
    dec,
    expected: np.ndarray | None,
    nbytes: int,
    repeats: int,
    note: str = "",
) -> FormatResult:
    try:
        blob = enc()
        decoded = dec(blob)
        lossless = True if expected is None else _equal(decoded, expected)
        e, _ = speed(enc, nbytes, repeats)
        d, _ = speed(lambda: dec(blob), nbytes, repeats)
        return FormatResult(name, len(blob), e / 1e6, d / 1e6, True, lossless, note)
    except Exception as exc:  # noqa: BLE001
        return FormatResult(name, 0, 0, 0, False, note=type(exc).__name__)


def compare_image(
    path: str | Path,
    repeats: int = 3,
    outer_codecs: tuple[str, ...] | list[str] = (DIF_TRIPLET[0],),
    palette_codecs: tuple[str, ...] | list[str] | None = (DIF_TRIPLET[1],),
    frame_codecs: tuple[str, ...] | list[str] | None = (DIF_TRIPLET[2],),
    stream: bool = False,
    num_threads: int = 1,
    index_widths: tuple[str, ...] | list[str] = ("auto",),
    dif_only: bool = False,
) -> list[FormatResult]:
    # Render `.drawio` to PNG once; every format encoder then sees the same
    # raster (PIL/imagecodecs can't open the drawio XML directly).
    raster = resolve_raster(path)
    # `load_image` returns an RGBA8 array; v3 DIF is indexed-only.
    arr = load_image(raster)
    rgb = _as_rgb(arr)
    nbytes = arr.nbytes
    rows: list[FormatResult] = []
    ref_size: int | None = None  # running reference for the live `rel` column

    def emit(name: str, enc, dec, expected, note: str = ""):
        """Measure one format, then print its row live (mirrors bench_image)."""
        nonlocal ref_size
        r = _measure(name, enc, dec, expected, nbytes, repeats, note)
        # PNG sets the running `rel` reference; the final table re-derives it via
        # _ref_size. dif rows stream before png, so their live rel stays blank
        # until png lands --- the re-derived final/TSV/report tables are correct.
        if ref_size is None and r.available and name == REL_REF:
            ref_size = r.size
        rows.append(r)
        if stream:
            print(_row_line(r, ref_size))

    # DIF encode is timed *raw bitmap -> file*: the palette/index build AND the
    # dark-theme synthesis run inside the closure (parity with png_encode(arr),
    # which encodes the raw array). `arr` is already in memory, so no file I/O
    # is timed. `decode` renders one theme back to pixels (file -> bitmap).
    # The whole `.dif` is encoded all-mt or all-st: workers come from --num-threads.
    workers = num_threads if num_threads > 1 else 0

    def dif_enc(strategy: str, outer: str, palette: str, frame: str, iw: str):
        return lambda: dif_image_from_array(arr, strategy, iw).to_dif(
            outer,
            palette,
            frame,
            workers=workers,
        )

    def dif_dec(b: bytes):
        return dif.Image.from_dif(b).render("light", (255, 255, 255), 0)[2]

    def dif_meta(iw: str) -> tuple[int, str]:
        """Resolve `(index_bits, note)` for width request `iw` from one untimed
        build --- the timed encoder rebuilds, so this only reads metadata. The note
        reports the pre-quantization color count when the palette was reduced."""
        img = dif_image_from_array(arr, "keep", iw)
        note = f"quant {img.source_colors}" if img.quantized() else ""
        return img.index_bits, note

    pcs = list(palette_codecs) if palette_codecs else [None]
    fcs = list(frame_codecs) if frame_codecs else [None]

    if dif_only:
        if stream:
            print(_DIF_HEAD)
            print(_DIF_SEP)

        for iw in index_widths:
            bits, note = dif_meta(iw)

            store_r = _measure(
                _dif_label("store", "store", "store", bits),
                dif_enc("keep", "store", "store", "store", iw),
                dif_dec,
                None,
                nbytes,
                repeats,
                note,
            )
            store_r.m = 0.0
            rows.append(store_r)
            if stream:
                print(_dif_row_line(store_r))

            for outer in outer_codecs:
                for pal in pcs:
                    for frm in fcs:
                        p = outer if pal is None else pal
                        f = outer if frm is None else frm
                        r = _measure(
                            _dif_label(outer, p, f, bits),
                            dif_enc("keep", outer, p, f, iw),
                            dif_dec,
                            None,
                            nbytes,
                            repeats,
                            note,
                        )
                        if (
                            store_r.available
                            and r.available
                            and r.enc_mbps > 0
                            and r.dec_mbps > 0
                        ):
                            r.m = compute_m(
                                store_r.size / r.size,
                                store_r.enc_mbps / r.enc_mbps,
                                store_r.dec_mbps / r.dec_mbps,
                            )
                        rows.append(r)
                        if stream:
                            print(_dif_row_line(r))

        return rows

    # The `rel` reference row (REL_REF picks it by name) is emitted first so every
    # row below has a live ratio and the table is topped by the baseline. png's
    # name and encoder are one unit; REL_REF only selects which row rel divides by.
    # FIXME: "emit ref first" assumes REL_REF == "png". If REL_REF moves to another
    # row, this hardcoded-first emit no longer matches it --- rows above the real ref
    # get a stale live ratio. Drive the first-emit off REL_REF, or assert they agree.
    if stream:
        print(_HEAD)
        print(_SEP)
    emit("png", lambda: imagecodecs.png_encode(arr), imagecodecs.png_decode, arr)

    # All DIF rows for one index-width request share its resolved width + quant
    # note (the palette --- hence the width --- is the same across codecs/themes).
    for iw in index_widths:
        bits, note = dif_meta(iw)

        # Headline row: the shipped triplet (store / zstd-16 / zstd-10) carrying
        # *both* themes (light + dark) --- the real `.dif` product, not directly
        # size-comparable to the single-theme formats below. `-2p` = 2-palette.
        outer, palette, frame = DIF_TRIPLET
        emit(
            f"{_dif_label(outer, palette, frame, bits)}-2p",
            dif_enc("arithmetic", outer, palette, frame, iw),
            dif_dec,
            None,
            note,
        )

        # Codec comparison: one theme each (apples-to-apples with the single-image
        # formats png/gif/webp/jxl/avif), encoded all-mt or all-st per --num-threads.
        # palette/frame default to inheriting the outer codec; given lists run the
        # cartesian product. DIF losslessness is covered in tests.
        for outer in outer_codecs:
            for pal in pcs:
                for frm in fcs:
                    p = outer if pal is None else pal
                    f = outer if frm is None else frm
                    emit(
                        _dif_label(outer, p, f, bits),
                        dif_enc("keep", outer, p, f, iw),
                        dif_dec,
                        None,
                        note,
                    )

    # avif/jxl pinned to their library's *native default* effort knob so the
    # comparison is reproducible: libjxl effort=7, libavif speed=6. (imagecodecs
    # leaves avif speed unset -> aom runs at speed 0, ~0.3 MB/s; we pin the
    # documented default instead.) webp keeps its own default. (png is emitted
    # above as the rel reference.) num-threads defaults to 1 so enc MB/s is an
    # apples-to-apples 1-core measure (dif/png/gif are single-threaded) and a
    # future imagecodecs `None`->auto default can't skew it; --num-threads N lifts
    # the cap to probe jxl/avif scaling (webp lossless ignores it).
    # The `3ch` note flags codecs fed `rgb` (alpha dropped) while enc/dec MB/s is
    # still normalized over `nbytes` = the 4-channel `arr`. They process 3/4 the
    # bytes but are credited the full RGBA payload, so their throughput reads ~4/3
    # high vs a strict bytes-processed measure. (png/dif/jxl encode `arr`, no skew.)
    nt = num_threads
    emit(
        "webp-ll",
        lambda: imagecodecs.webp_encode(rgb, lossless=True, numthreads=nt),
        imagecodecs.webp_decode,
        rgb,
        note="3ch",
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
        note="3ch",
    )

    # GIF via Pillow (lossless LZW; round-trips exactly only for <=256 colors,
    # else the palette quantize drops colors --- a width limit, not codec loss).
    pil = PILImage.fromarray(rgb)

    def gif_enc() -> bytes:
        buf = io.BytesIO()
        pil.quantize(colors=256).save(buf, format="GIF")
        return buf.getvalue()

    def gif_dec(b: bytes) -> np.ndarray:
        out = PILImage.open(io.BytesIO(b))
        return np.asarray(out.convert("RGB"))

    emit("gif", gif_enc, gif_dec, rgb, note="3ch")
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
_FMT_W = 25
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
    return (
        f"| {r.name:^{_FMT_W}} | {r.size:>{_SIZE_W}} | {r.enc_mbps:>{_SPD_W}.1f} "
        f"| {r.dec_mbps:>{_SPD_W}.1f} | {rel:>{_REL_W}} | {r.note:^{_NOTE_W}} |"
    )


# DIF-only mode display (M metric relative to store baseline).
_DIF_M_W = 8
_DIF_HEAD = (
    f"| {'format':^{_FMT_W}} | {'size':^{_SIZE_W}} | {'enc MB/s':^{_SPD_W}} "
    f"| {'dec MB/s':^{_SPD_W}} | {'M':^{_DIF_M_W}} | {'note':^{_NOTE_W}} |"
)
_DIF_SEP = (
    f"|{'-' * (_FMT_W + 2)}|{'-' * (_SIZE_W + 2)}|{'-' * (_SPD_W + 2)}"
    f"|{'-' * (_SPD_W + 2)}|{'-' * (_DIF_M_W + 2)}|{'-' * (_NOTE_W + 2)}|"
)


def _dif_row_line(r: FormatResult) -> str:
    if not r.available:
        return (
            f"| {r.name:^{_FMT_W}} | {'n/a':^{_SIZE_W}} | {'':^{_SPD_W}} "
            f"| {'':^{_SPD_W}} | {'':^{_DIF_M_W}} | {r.note:^{_NOTE_W}} |"
        )
    m_str = f"{r.m:.2f}" if r.m == r.m else ""  # nan check
    return (
        f"| {r.name:^{_FMT_W}} | {r.size:>{_SIZE_W}} | {r.enc_mbps:>{_SPD_W}.1f} "
        f"| {r.dec_mbps:>{_SPD_W}.1f} | {m_str:>{_DIF_M_W}} | {r.note:^{_NOTE_W}} |"
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
    "M",
    "available",
    "note",
)


def iter_rows(path: str | Path, rows: list[FormatResult]):
    """One flat row per (image, format) for CSV/TSV export."""
    name = Path(path).name
    ref_size = _ref_size(rows)
    for r in rows:
        rel = f"{r.size / ref_size:.4f}" if (ref_size and r.available) else ""
        m_val = f"{r.m:.4f}" if (r.available and r.m == r.m) else ""
        yield (
            name,
            r.name,
            r.size if r.available else "",
            f"{r.enc_mbps:.2f}" if r.available else "",
            f"{r.dec_mbps:.2f}" if r.available else "",
            rel,
            m_val,
            int(r.available),
            r.note,
        )


# --- Aggregate stats (mirrors bench/runner.py's DirStat / subdir_stats) ------
# One row per image is detail (console + TSV); the report aggregates every
# format over all images beneath each directory, recursively.

# (path, rows) for one image --- the unit aggregation buckets over.
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
    m_mean: float = float("nan")
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
        ms = [r.m for r in rs if r.m == r.m]  # exclude nan
        stats.append(
            FormatStat(
                name,
                len(rs),
                statistics.mean(r.size for r in rs),
                statistics.mean(r.enc_mbps for r in rs),
                statistics.mean(r.dec_mbps for r in rs),
                statistics.mean(rels) if rels else float("nan"),
                statistics.mean(ms) if ms else float("nan"),
                rs[-1].note,
            )
        )
    # dif_only mode: sort by M mean descending; otherwise rel reference first.
    has_m = any(s.m_mean == s.m_mean for s in stats)
    if has_m:
        stats.sort(key=lambda s: (s.m_mean != s.m_mean, -s.m_mean))
    else:
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
    has_m = any(s.m_mean == s.m_mean for s in stats)
    if has_m:
        rows = [
            "| format | n | size | enc MB/s | dec MB/s | M mean | note |",
            "|---|--:|--:|--:|--:|--:|---|",
        ]
        for s in stats:
            m_str = "" if s.m_mean != s.m_mean else f"{s.m_mean:.3f}"
            rows.append(
                f"| {s.name} | {s.n} | {s.size_mean:.0f} | {s.enc_mbps:.1f} "
                f"| {s.dec_mbps:.1f} | {m_str} | {s.note} |"
            )
    else:
        rows = [
            "| format | n | size | enc MB/s | dec MB/s | rel | note |",
            "|---|--:|--:|--:|--:|--:|---|",
        ]
        for s in stats:
            rel = "" if s.rel_mean != s.rel_mean else f"x{s.rel_mean:.2f}"  # nan guard
            rows.append(
                f"| {s.name} | {s.n} | {s.size_mean:.0f} | {s.enc_mbps:.1f} "
                f"| {s.dec_mbps:.1f} | {rel} | {s.note} |"
            )
    return "\n".join(rows)
