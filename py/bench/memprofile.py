"""Per-run memory profiling for the codec / format benchmarks.

The benchmark reports two memory stats --- ``max`` (peak) and ``mean`` --- as the
working-set *growth* a codec/encoder forces over the entry baseline. They are read
from jemalloc's ``stats.allocated`` counter via ``ctypes`` / ``mallctl``:

- jemalloc must be ``LD_PRELOAD``ed (the ``just bench-*`` recipes do this). Without
  it :class:`track_memory` reports ``None`` and the tables render a blank cell.
- ``stats.allocated`` is the live "bytes currently allocated by the application": it
  rises on ``malloc`` and falls on ``free``, so a poll thread sampling it gives a
  clean high-water (max) and time-average (mean) for *this* block --- no
  cross-run contamination (unlike a process RSS high-water the OS never reclaims).
- it captures native allocations (libbsc / lzav / kanzi) and worker threads (zstd
  ``-mt``), since every allocation routes through the preloaded allocator.

Why jemalloc and not mimalloc: the distro mimalloc release build compiles its stat
counters out (``mi_process_info`` returns 0 for commit), and its only moving figure
(``peak_rss``) is the non-resettable process-lifetime OS high-water. jemalloc's
``stats.allocated`` works in the stock package.

``mallctl`` needs an ``epoch`` write before each read to refresh the cached stats.
"""

from __future__ import annotations

import ctypes
import statistics
import threading

_MB = 1048576


class _Jemalloc:  # pragma: no cover - only runs with libjemalloc preloaded
    """``ctypes`` wrapper over jemalloc's ``mallctl`` ``stats.allocated`` counter."""

    def __init__(self, mallctl) -> None:
        self._mallctl = mallctl

    def allocated(self) -> int:
        """Bytes currently allocated by the application (live, post-epoch-refresh)."""
        epoch = ctypes.c_uint64(1)
        self._mallctl(b"epoch", None, None, ctypes.byref(epoch), ctypes.sizeof(epoch))
        out = ctypes.c_size_t(0)
        olen = ctypes.c_size_t(ctypes.sizeof(out))
        self._mallctl(
            b"stats.allocated", ctypes.byref(out), ctypes.byref(olen), None, 0
        )
        return out.value


_probed = False
_probe: _Jemalloc | None = None


def _jemalloc() -> _Jemalloc | None:
    """The jemalloc probe if ``libjemalloc`` is preloaded, else ``None`` (cached).

    ``mallctl`` lives in the process global symbol table only when jemalloc is
    ``LD_PRELOAD``ed; glibc has no such symbol, so a normal build/test run raises
    ``AttributeError`` and this returns ``None`` (profiling becomes a no-op).
    """
    global _probed, _probe
    if _probed:
        return _probe
    _probed = True
    try:
        lib = ctypes.CDLL(None)
        mallctl = lib.mallctl
    except (AttributeError, OSError):
        return None
    mallctl.restype = ctypes.c_int  # pragma: no cover - needs jemalloc preloaded
    _probe = _Jemalloc(mallctl)  # pragma: no cover - needs jemalloc preloaded
    return _probe  # pragma: no cover - needs jemalloc preloaded


class track_memory:
    """Context manager: peak + mean allocation *growth* over the wrapped block.

    A background thread samples jemalloc's ``stats.allocated`` every ``interval``
    seconds; ``max_mb`` / ``mean_mb`` are the high-water / time-average minus the
    entry baseline, in MB. Reports ``None`` (-> blank cell) when jemalloc is not
    preloaded.
    """

    def __init__(self, interval: float = 0.001) -> None:
        self._interval = interval
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._probe: _Jemalloc | None = None
        self._base = 0
        self._samples: list[int] = []
        self._max: float | None = None
        self._mean: float | None = None

    def __enter__(self) -> "track_memory":
        self._probe = _jemalloc()
        if self._probe is not None:
            self._base = self._probe.allocated()
            self._samples = [self._base]
            self._thread = threading.Thread(target=self._poll, daemon=True)
            self._thread.start()
        return self

    def _poll(self) -> None:
        assert self._probe is not None
        while not self._stop.wait(self._interval):
            self._samples.append(self._probe.allocated())

    def __exit__(self, *_exc: object) -> None:
        if self._thread is not None:
            self._stop.set()
            self._thread.join()
        if self._probe is not None:
            self._max = max(0, max(self._samples) - self._base) / _MB
            self._mean = max(0.0, statistics.mean(self._samples) - self._base) / _MB

    @property
    def max_mb(self) -> float | None:
        return self._max

    @property
    def mean_mb(self) -> float | None:
        return self._mean
