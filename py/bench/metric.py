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
import os
import threading
import time
from typing import Callable


def _rss_bytes() -> int:
    """Process resident set size in bytes, or 0 if unavailable (non-Linux)."""
    try:
        with open("/proc/self/statm") as f:
            resident_pages = int(f.read().split()[1])
        return resident_pages * os.sysconf("SC_PAGE_SIZE")
    except (OSError, ValueError, IndexError):  # pragma: no cover - platform-dependent
        return 0


class peak_rss:
    """Context manager sampling peak RSS *over baseline* while the block runs.

    A daemon thread polls ``/proc/self/statm`` so it captures memory allocated by
    C/C++ codecs (libbsc, lzav, kanzi) that ``tracemalloc`` can't see. ``.delta``
    is the high-water RSS minus the entry baseline, in bytes (0 if unsupported).

    Caveat: RSS is a *process* high-water that the allocator rarely returns, so a
    codec run right after a hungrier one can under-report (its working set fits in
    pages already resident). Read ``.delta`` as the *new* growth this block forced,
    not an isolated footprint; the first/largest codec of a kind is the honest one.
    """

    def __init__(self, interval: float = 0.001) -> None:
        self._interval = interval
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._base = 0
        self._peak = 0

    def __enter__(self) -> "peak_rss":
        self._base = self._peak = _rss_bytes()
        if self._base:
            self._thread = threading.Thread(target=self._poll, daemon=True)
            self._thread.start()
        return self

    def _poll(self) -> None:
        while not self._stop.wait(self._interval):
            r = _rss_bytes()
            if r > self._peak:
                self._peak = r

    def __exit__(self, *_exc: object) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join()

    @property
    def delta(self) -> int:
        return max(0, self._peak - self._base)


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
