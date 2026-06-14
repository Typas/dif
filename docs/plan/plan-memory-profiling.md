# Plan: add memory profiling feature on `just bench-codecs` and `just bench-formats`

## Goal
`just bench-codecs` reported a `peak MB` column, but the number was meaningless: it
polled process RSS from `/proc/self/statm`, and RSS is a process high-water the
allocator rarely returns to the OS. A codec run after a hungrier one under-reported
(its working set fit in pages already resident), so the column was contaminated by
run order. `just bench-formats` reported no memory at all.

Delivered: profile memory correctly and report two stats per codec/format ---
`max MB` (peak) and `mean MB` --- both as the working-set *growth* the codec/encoder
forces, isolated per run (no cross-run contamination). All new code is in Python.

## Decisions
- Mechanism: query an allocator's live "currently allocated" counter (rises on
  `malloc`, falls on `free`) and sample it in a poll thread for max + mean. Not
  memray (per-codec capture files are heavy and "mean" is awkward to derive).
- Allocator: **jemalloc**. Its `stats.allocated` (via `mallctl`) is the bytes
  currently allocated by the application; it tracks alloc *and* free, is
  process-wide (captures zstd `-mt` workers and native codecs), and works in the
  stock distro package.
  - Why not mimalloc (originally chosen, then rejected at verification): the distro
    mimalloc release build compiles its stat counters out, so `mi_process_info`
    returns 0 for `current_commit`/`peak_commit` and `current_rss` (verified by
    direct probe). Its only moving figure, `peak_rss`, is the OS `ru_maxrss`
    process-lifetime high-water and is *not* reset by `mi_stats_reset` --- i.e. the
    same contamination flaw. No runtime/env toggle re-enables the counters (it is
    compile-time `MI_STAT`), and no dnf mimalloc package ships them.
  - tcmalloc: rejected (C++ `MallocExtension`, ABI-fragile via `ctypes`).
- Semantics: **delta over entry baseline** --- growth caused by the codec, excluding
  the input already live in memory.
- Columns: two new columns, `max MB` and `mean MB`, in *both* `bench-codecs` and
  `bench-formats` (console table, TSV, markdown aggregate). They replace the single
  `peak MB` column in `bench-codecs`; `bench-formats` had none.
- Fallback: when jemalloc is not preloaded (plain `pytest`, or a forgotten
  `LD_PRELOAD`), the memory cells render blank (`-`). The benchmark still runs;
  speeds/ratios are unaffected. The `just` recipes set `LD_PRELOAD` so the real
  numbers appear there.

## Allocator
- [jemalloc](https://github.com/jemalloc/jemalloc) --- the chosen allocator.
  - `int mallctl(const char *name, void *oldp, size_t *oldlenp, void *newp,`
    `size_t newlen)` --- write `epoch` (a `uint64`) to refresh the cached stats,
    then read `stats.allocated` (a `size_t`) for the live allocated-bytes figure.
  - stats are on in the stock build (`--enable-stats` default); verified moving:
    base 1 MB -> +160 MB after a 128 MB allocation -> back down after free.
- mimalloc / tcmalloc: considered and rejected (see Decisions).

## Profiling tools
- memray: considered and rejected (heavy per-region capture files; "mean" awkward).
- The allocator-stats approach above is the profiler.

## Python rewiring

### New file: `py/bench/memprofile.py`
Isolates the jemalloc binding from `metric.py` (which keeps the `M`-metric and
timing code).

- `_jemalloc()` -> probe | `None` (cached). Resolve `mallctl` off
  `ctypes.CDLL(None)` (the process's global symbol table --- `mallctl` exists only
  when jemalloc is `LD_PRELOAD`ed; glibc has no such symbol).
  `AttributeError`/`OSError` -> `None`. Kept tiny: the jemalloc-present branch is
  the only part unreachable without jemalloc loaded (`# pragma: no cover`).
  - `class _Jemalloc` (also `# pragma: no cover`) wraps `allocated()`: write `epoch`
    to refresh, then read `stats.allocated`.
- `class track_memory` (context manager, replaces the old `metric.peak_rss`):
  - `__enter__`: `probe = _jemalloc()`. If `None`, no-op (unsupported). Else
    `self._base = probe.allocated()`, `self._samples = [self._base]`, start a daemon
    thread polling `probe.allocated()` every `interval` into `self._samples`.
  - `__exit__`: stop + join the thread. If supported:
    `self._max = max(0, max(samples) - base) / MB`;
    `self._mean = max(0.0, mean(samples) - base) / MB`.
  - `max_mb` / `mean_mb` properties -> `float` MB, or `None` when unsupported.
- Deleted `peak_rss`, `_rss_bytes` from `metric.py`; `memcpy_speed`, `speed`,
  `_best_time`, `compute_m` stay.

### `py/bench/runner.py` (bench-codecs)
- Import `track_memory` from `.memprofile` instead of `peak_rss` from `.metric`.
- `CodecResult`: `peak_mb` replaced by `max_mb: float | None` + `mean_mb: float | None`.
- `bench_image`: wrap the single untimed compress+decompress in `track_memory()`;
  read `pk.max_mb` / `pk.mean_mb`. Two memory columns; `None` -> `-` (helper `_mb`).
- `DirStat`: `max_mb`/`mean_mb`; `_aggregate` averages each over available results,
  skipping `None` (helper `_avg_opt`; blank if all `None`).
- `format_table`, `format_stats_table`, `TSV_HEADER`, `iter_rows`: two columns
  (blank string in TSV when `None`).

### `py/bench/compare.py` (bench-formats)
- `FormatResult` / `FormatStat`: add `max_mb` / `mean_mb`.
- `_measure`: wrap the untimed `enc()` + `dec(blob)` in `track_memory()`; store the
  two values. (Timed `speed(...)` passes stay outside the tracker, so profiling
  never distorts timing.)
- `_HEAD`/`_SEP`/`_row_line`, `_DIF_*`, `format_table`, `format_stats_table`,
  `TSV_HEADER`, `iter_rows`, `_aggregate`: add the two columns (`-`/blank when `None`).

## Justfile rewiring
- Resolve the installed jemalloc shared object via `ldconfig` (handles a versioned
  soname like `libjemalloc.so.2` when the bare `.so` from `-devel` is absent; empty
  if none installed -> `LD_PRELOAD` stays empty -> columns blank, benchmark runs):
  `jemalloc := \`ldconfig -p 2>/dev/null | awk -F'=> ' '/libjemalloc/{print $2; exit}'\``
- Prefix the two bench recipes, defaulting `LD_PRELOAD` but yielding to a user-set
  one (a user can point at any path/allocator via the standard variable):
  - `bench-codecs *ARGS:` ->
    `LD_PRELOAD="${LD_PRELOAD:-{{jemalloc}}}" uv run python -m bench codecs {{ARGS}}`
  - `bench-formats *ARGS:` ->
    `LD_PRELOAD="${LD_PRELOAD:-{{jemalloc}}}" uv run python -m bench formats {{ARGS}}`
- `bench-setup` unchanged (jemalloc is a runtime preload, not a built shim). Recipe
  comments note `dnf install jemalloc` (or `LD_PRELOAD` to your own build) is needed
  for real numbers, and that the columns blank out otherwise.

## Testing
- `py/tests/test_memprofile.py`:
  - Unsupported path: `_jemalloc()` -> `None` in plain pytest; `track_memory` yields
    `max_mb`/`mean_mb` `None`.
  - Supported path: a `_FakeProbe(_Jemalloc)` subclass with scripted `allocated()`
    readings (so it is type-assignable to `track_memory._probe`) drives both the
    threaded poll path and a direct `__exit__` math check (max/mean/baseline/MB,
    plus the negative-clamp) without jemalloc installed.
- `test_bench.py` / `test_compare.py`: assert the new columns + `_mb`/`_avg_opt`
  helpers, and that cells are blank without jemalloc; fixed the moved TSV
  `available` index in `test_compare`.
- Gate: `just py-ci` (fmt + lint + test + per-file >= 80% cov). `memprofile.py`
  reaches 100% (fake probe carries the logic; the jemalloc-only glue is pragma'd).

## Out of scope
- Per-allocation backtraces / leak profiling (jemalloc `prof` / memray) --- not
  needed for max/mean numbers.
- mimalloc / tcmalloc support.
- Changing the `M` metric (memory is reported, not folded into `M`).
