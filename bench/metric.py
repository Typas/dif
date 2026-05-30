"""Timing utilities and the project's codec-quality metric ``M``.

``M = log(5S/4) - log(C/4) - log(D)`` where, against a ``memcpy`` baseline:

- ``S = size_original / size_compressed`` (compression ratio, higher better),
- ``C = memcpy_speed / compress_speed`` (compress slowdown, lower better),
- ``D = memcpy_speed / decompress_speed`` (decompress slowdown, lower better).

Higher ``M`` is better. Speeds are bytes/sec measured over the *original*
payload so the three ratios are dimensionless.
"""

from __future__ import annotations

import math
import time
from typing import Callable


def _best_time(fn: Callable[[], object], repeats: int) -> float:
    fn()  # warmup
    best = math.inf
    for _ in range(repeats):
        t0 = time.perf_counter()
        fn()
        best = min(best, time.perf_counter() - t0)
    return best


def memcpy_speed(data: bytes, repeats: int = 7) -> float:
    """Bytes/sec for a plain copy of ``data`` (the metric's baseline)."""
    t = _best_time(lambda: bytes(data), repeats)
    return len(data) / t if t > 0 else math.inf


def speed(fn: Callable[[], object], nbytes: int, repeats: int = 5) -> float:
    """Bytes/sec for ``fn`` processing ``nbytes`` of original data."""
    t = _best_time(fn, repeats)
    return nbytes / t if t > 0 else math.inf


def compute_m(ratio_s: float, ratio_c: float, ratio_d: float) -> float:
    return math.log(5 * ratio_s / 4) - math.log(ratio_c / 4) - math.log(ratio_d)
