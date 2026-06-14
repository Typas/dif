"""Smoke tests for the benchmark harness."""

from __future__ import annotations

import math

from bench.codecs import available_codecs
from bench.metric import compute_m, memcpy_speed, speed
from bench.runner import (
    TSV_HEADER,
    _avg_opt,
    _mb,
    format_stats_table,
    format_table,
    iter_rows,
    run,
    subdir_stats,
)


def test_compute_m_rewards_ratio_and_speed():
    # Higher ratio -> higher M; slower codec (bigger C/D) -> lower M.
    fast_small = compute_m(3.0, 2.0, 2.0)
    slow_small = compute_m(3.0, 50.0, 50.0)
    big_ratio = compute_m(6.0, 2.0, 2.0)
    assert fast_small > slow_small
    assert big_ratio > fast_small
    assert math.isfinite(fast_small)


def test_codecs_registry_roundtrips():
    codecs = available_codecs()
    assert codecs, "expected at least the pure-python codecs"
    data = bytes(range(256)) * 64
    for c in codecs:
        comp = c.compress(data)
        assert c.decompress(comp, len(data)) == data, c.name


def test_speed_helpers_positive():
    data = b"x" * 100_000
    assert memcpy_speed(data, repeats=2) > 0
    assert speed(lambda: bytes(data), len(data), repeats=2)[0] > 0


def _toy_png(path):
    import numpy as np
    from PIL import Image as PILImage

    arr = np.zeros((16, 16, 4), np.uint8)
    arr[..., 3] = 255
    arr[:8, :8, :3] = (200, 30, 40)
    PILImage.fromarray(arr, "RGBA").save(path)
    return path


def test_run_over_small_image(tmp_path):
    p = _toy_png(tmp_path / "d.png")
    reports = run([p], repeats=2)
    assert len(reports) == 1
    rep = reports[0]
    assert rep.memcpy_mbps > 0 and rep.difr_bytes > 0
    best = rep.results[0]
    assert best.available and math.isfinite(best.m)
    assert best.c > 0 and best.d > 0


def test_memory_columns_and_helpers(tmp_path):
    # mimalloc is not preloaded in the test process, so the memory columns exist
    # but every cell is blank (max_mb / mean_mb are None end to end).
    assert _mb(None) == "-"
    assert _mb(2.34) == "2.3"
    assert _avg_opt([None, None]) is None
    assert _avg_opt([2.0, None, 4.0]) == 3.0

    p = _toy_png(tmp_path / "d.png")
    reports = run([p], repeats=2)
    rep = reports[0]
    assert all(r.max_mb is None and r.mean_mb is None for r in rep.results)

    table = format_table(rep.results)
    assert "max MB" in table and "mean MB" in table

    assert "max_mb" in TSV_HEADER and "mean_mb" in TSV_HEADER
    mx, mn = TSV_HEADER.index("max_mb"), TSV_HEADER.index("mean_mb")
    rows = list(iter_rows(reports))
    assert rows and all(t[mx] == "" and t[mn] == "" for t in rows)

    blocks = subdir_stats(reports)
    _, stats = blocks[0]
    md = format_stats_table(stats)
    assert "max MB" in md and "mean MB" in md
    assert all(s.max_mb is None and s.mean_mb is None for s in stats)


def test_subdir_stats_aggregates_recursively(tmp_path):
    sub = tmp_path / "a" / "b"
    sub.mkdir(parents=True)
    reports = run(
        [_toy_png(tmp_path / "a" / "one.png"), _toy_png(sub / "two.png")], repeats=2
    )
    blocks = dict(subdir_stats(reports))
    # Leaf dir sees 1 image; the parent that contains both sees 2.
    counts = {label: max(s.n for s in stats) for label, stats in blocks.items()}
    assert max(counts.values()) == 2
    assert min(counts.values()) == 1
