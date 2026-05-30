"""Smoke tests for the benchmark harness."""

from __future__ import annotations

import math

from bench.codecs import available_codecs
from bench.metric import compute_m, memcpy_speed, speed
from bench.runner import run


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
    assert speed(lambda: bytes(data), len(data), repeats=2) > 0


def test_run_over_small_image(tmp_path):
    import numpy as np
    from PIL import Image as PILImage

    arr = np.zeros((16, 16, 4), np.uint8)
    arr[..., 3] = 255
    arr[:8, :8, :3] = (200, 30, 40)
    p = tmp_path / "d.png"
    PILImage.fromarray(arr, "RGBA").save(p)
    results, payloads = run([p], repeats=2)
    assert payloads and results
    best = results[0]
    assert best.available and math.isfinite(best.m)
