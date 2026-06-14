"""Timing utilities and the project's codec-quality metric ``M``.

``M = 4*log(S) - log(C)/2 - log(D)`` where, against a ``memcpy`` baseline:

- ``S = size_original / size_compressed`` (compression ratio, higher better),
- ``C = memcpy_speed / compress_speed`` (compress slowdown, lower better),
- ``D = memcpy_speed / decompress_speed`` (decompress slowdown, lower better).

Higher ``M`` is better. Speeds are bytes/sec measured over the *original*
payload so the three ratios are dimensionless.
"""

from __future__ import annotations
import ctypes

import math
import time
from typing import Callable


def _best_time(fn: Callable[[], bytes], repeats: int) -> tuple[float, bytes]:
    obj = fn()  # warmup
    best = math.inf
    for _ in range(repeats):
        t0 = time.perf_counter()
        fn()
        best = min(best, time.perf_counter() - t0)
    return best, obj


def memcpy_speed(data: bytes, repeats: int = 7) -> float:
    """Bytes/sec for a plain copy of ``data`` (the metric's baseline)."""
    buffer = ctypes.create_string_buffer(data)

    def run_memcpy() -> bytes:
        ctypes.memmove(buffer, data, len(data))
        return data

    t, _ = _best_time(run_memcpy, repeats)
    return len(data) / t if t > 0 else -math.inf


def speed(
    fn: Callable[[], bytes], nbytes: int, repeats: int = 5
) -> tuple[float, bytes]:
    """Bytes/sec for ``fn`` processing ``nbytes`` of original data."""
    t, obj = _best_time(fn, repeats)
    return nbytes / t if t > 0 else -math.inf, obj


def compute_m(ratio_s: float, ratio_c: float, ratio_d: float) -> float:
    return 4 * math.log(ratio_s) - math.log(ratio_c) / 2 - math.log(ratio_d)
