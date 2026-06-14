"""Memory profiler (``bench.memprofile``).

jemalloc is not preloaded in the pytest process, so the live ``_jemalloc`` probe
returns ``None`` (the supported math is exercised via a fake probe instead).
"""

from __future__ import annotations

import time

import bench.memprofile as mp
from bench.memprofile import _MB, _Jemalloc, track_memory


def test_jemalloc_probe_is_none_without_preload():
    # No libjemalloc preloaded -> mallctl symbol absent in glibc -> None.
    mp._probed = False
    mp._probe = None
    assert mp._jemalloc() is None
    # Second call hits the cached value (still None).
    assert mp._jemalloc() is None


def test_track_memory_reports_none_when_unsupported():
    with track_memory() as pk:
        b = bytes(1024) * 16  # noqa: F841 - allocate something to be ignored
    assert pk.max_mb is None
    assert pk.mean_mb is None


class _FakeProbe(_Jemalloc):
    """Stand-in for the jemalloc probe with scripted ``allocated`` readings.

    Subclasses ``_Jemalloc`` (so it is assignable to ``track_memory._probe``) but
    overrides ``allocated`` --- it never touches ctypes, hence no ``super().__init__``.
    """

    def __init__(self, allocs: list[int]) -> None:
        self._allocs = list(allocs)
        self._i = 0

    def allocated(self) -> int:
        v = self._allocs[min(self._i, len(self._allocs) - 1)]
        self._i += 1
        return v


def test_track_memory_threaded_path(monkeypatch):
    # base = first allocated() = 10 MB; the poll thread then sees 30 MB.
    fake = _FakeProbe(allocs=[10 * _MB, 30 * _MB])
    monkeypatch.setattr(mp, "_jemalloc", lambda: fake)
    with track_memory(interval=0.0005) as pk:
        time.sleep(0.01)  # let the poll thread sample at least once
    mx, mn = pk.max_mb, pk.mean_mb
    assert mx == 20.0  # (30 - 10) MB high-water
    assert mn is not None and mx is not None
    assert 0.0 <= mn <= mx


def test_track_memory_exit_math_deterministic():
    # Drive __exit__ directly (no thread) for exact max/mean over fixed samples.
    fake = _FakeProbe(allocs=[2 * _MB])
    tm = track_memory()
    tm._probe = fake
    tm._base = 2 * _MB
    tm._samples = [2 * _MB, 6 * _MB, 10 * _MB]  # max = 10 MB, mean = 6 MB
    tm._thread = None
    tm.__exit__()
    assert tm.max_mb == 8.0  # (10 - 2) MB
    assert tm.mean_mb == 4.0  # (6 - 2) MB


def test_track_memory_clamps_negative_to_zero():
    # samples below baseline (allocator freed past entry) -> clamped, never negative.
    fake = _FakeProbe(allocs=[5 * _MB])
    tm = track_memory()
    tm._probe = fake
    tm._base = 5 * _MB
    tm._samples = [5 * _MB, 1 * _MB]  # max == base, mean below base
    tm._thread = None
    tm.__exit__()
    assert tm.max_mb == 0.0
    assert tm.mean_mb == 0.0
