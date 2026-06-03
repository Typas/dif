"""Format-comparison benchmark: DIF vs PNG/WebP/JXL/AVIF/GIF over a toy image."""

from __future__ import annotations

import numpy as np
from PIL import Image as PILImage

from bench.compare import (
    DIF_BASELINE,
    REL_REF,
    _abbr,
    _dif_label,
    compare_image,
    format_stats_table,
    format_table,
    iter_rows,
    subdir_stats,
)


def _toy_png(path):
    arr = np.zeros((16, 16, 4), np.uint8)
    arr[..., 3] = 255
    arr[:8, :8, :3] = (200, 30, 40)
    arr[8:, 8:, :3] = (30, 90, 200)
    PILImage.fromarray(arr, "RGBA").save(path)
    return path


def test_compare_image_covers_png_and_dif(tmp_path):
    p = _toy_png(tmp_path / "d.png")
    rows = compare_image(p, repeats=1, dif_codecs=("zstd-3", "brotli-5"))
    by_name = {r.name: r for r in rows}

    # PNG is the rel reference: present, lossless, non-empty.
    png = by_name[REL_REF]
    assert png.available and png.lossless and png.size > 0

    # The shipped 2-theme headline row + the requested single-theme codec rows.
    base = _dif_label(DIF_BASELINE, DIF_BASELINE, DIF_BASELINE)  # "dif-zst3"
    assert f"{base}-2t" in by_name
    assert by_name[base].available and by_name[base].size > 0

    # Every available row that claims lossless must round-trip losslessly.
    assert all(r.lossless for r in rows if r.available)


def test_dif_label_abbreviations():
    assert _abbr("zstd-3") == "zst3"
    assert _abbr("zstd-10") == "zst10"
    assert _abbr("brotli-5") == "br5"
    assert _abbr("libdeflate-6") == "PK6"
    assert _abbr("lz4-fast1") == "4f1"
    assert _abbr("lz4-hc9") == "4hc9"
    assert _abbr("lzav-1") == "av1"
    assert _abbr("zxc-3") == "zxc3"
    assert _abbr("store") == "st"
    assert _dif_label("zstd-3", "zstd-3", "zstd-3") == "dif-zst3"
    assert _dif_label("zstd-3", "store", "libdeflate-6") == "dif-zst3-st-PK6"


def test_dif_palette_frame_cartesian(tmp_path):
    p = _toy_png(tmp_path / "d.png")
    rows = compare_image(
        p,
        repeats=1,
        dif_codecs=("zstd-3",),
        palette_codecs=("store", "libdeflate-6"),
        frame_codecs=("store",),
    )
    names = {r.name for r in rows}
    assert "dif-zst3-st-st" in names
    assert "dif-zst3-PK6-st" in names


def test_unavailable_codec_is_annotated_not_raised(tmp_path):
    # A bogus DIF codec name fails inside the measured closure; the row comes
    # back available=False with a note instead of bubbling an exception.
    p = _toy_png(tmp_path / "d.png")
    rows = compare_image(p, repeats=1, dif_codecs=("not-a-codec",))
    bad = next(r for r in rows if r.name == "dif-not-a-codec")
    assert not bad.available and bad.note


def test_tables_and_tsv_rows(tmp_path):
    p = _toy_png(tmp_path / "d.png")
    rows = compare_image(p, repeats=1, dif_codecs=("zstd-3",))

    table = format_table(p, rows)
    assert "format" in table and "png" in table

    tsv = list(iter_rows(p, rows))
    assert len(tsv) == len(rows)
    assert all(len(t) == 9 for t in tsv)  # matches compare.TSV_HEADER width


def test_subdir_stats_aggregates(tmp_path):
    a = _toy_png(tmp_path / "one.png")
    b = _toy_png(tmp_path / "two.png")
    reports = [
        (str(a), compare_image(a, repeats=1, dif_codecs=("zstd-3",))),
        (str(b), compare_image(b, repeats=1, dif_codecs=("zstd-3",))),
    ]
    blocks = subdir_stats(reports)
    assert blocks
    label, stats = blocks[0]
    assert stats and stats[0].name == REL_REF  # png sorts first
    assert stats[0].n == 2  # aggregated over both images
    md = format_stats_table(stats)
    assert "| format |" in md
