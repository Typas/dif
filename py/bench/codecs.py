"""Registry of lossless compression codecs for the `.difr` benchmark.

This benchmark measures *standalone* compression algorithms over the raw `.difr`
body — it deliberately does **not** include the DIF container itself. Each
:class:`Codec` self-detects availability at import time, so the harness runs with
whatever is installed and reports the rest as unavailable. ``decompress`` receives
the original length because some libraries (libdeflate) require the output size up
front.

Codecs are thread-aware: a codec may carry an optional multithreaded encoder.
:func:`all_codecs` picks the multithreaded encoder when ``numthreads > 1`` (and the
codec has one), else the single-thread encoder — never both for one codec.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable


@dataclass
class Codec:
    name: str
    compress: Callable[[bytes], bytes]
    decompress: Callable[[bytes, int], bytes]
    available: bool = True
    note: str = ""
    # Optional multithreaded encoder `(data, numthreads) -> bytes`; the stream
    # decodes identically with `decompress`. None = single-thread only.
    mt_compress: Callable[[bytes, int], bytes] | None = None


_REGISTRY: list[Codec] = []


def _add(codec: Codec) -> None:
    _REGISTRY.append(codec)


def _unavailable(name: str, note: str) -> Codec:
    def _na(*_a, **_k):  # pragma: no cover - never called
        raise RuntimeError(f"{name} unavailable: {note}")

    return Codec(name, _na, _na, available=False, note=note)


# --- libdeflate (baseline) ------------------------------------------------
try:
    import deflate as _deflate

    _add(
        Codec(
            "libdeflate-6",
            lambda d: bytes(_deflate.deflate_compress(d, 6)),
            lambda c, n: bytes(_deflate.deflate_decompress(c, n)),
        )
    )
except ImportError:  # pragma: no cover
    _add(_unavailable("libdeflate-6", "pip install deflate"))

# --- brotli ---------------------------------------------------------------
try:
    import brotli as _brotli

    _add(
        Codec(
            "brotli-5",
            lambda d: _brotli.compress(d, quality=5),
            lambda c, n: _brotli.decompress(c),
        )
    )
except ImportError:  # pragma: no cover
    _add(_unavailable("brotli", "pip install brotli"))

# --- lz4 ------------------------------------------------------------------
try:
    import lz4.block as _lz4b

    _add(
        Codec(
            "lz4-fast1",
            lambda d: _lz4b.compress(d, mode="fast", acceleration=1, store_size=True),
            lambda c, n: _lz4b.decompress(c),
        )
    )
    for _lvl in (4, 9):
        _add(
            Codec(
                f"lz4-hc{_lvl}",
                (
                    lambda lv: (
                        lambda d: _lz4b.compress(
                            d, mode="high_compression", compression=lv, store_size=True
                        )
                    )
                )(_lvl),
                lambda c, n: _lz4b.decompress(c),
            )
        )
except ImportError:  # pragma: no cover
    _add(_unavailable("lz4", "pip install lz4"))

# --- zstd (via pyzstd; fast = negative level). Multithreaded via nbWorkers. ----
try:
    import pyzstd as _pyzstd
    from pyzstd import CParameter as _CP

    def _zstd_mt(lvl: int):
        def mt(data: bytes, nt: int) -> bytes:
            return _pyzstd.compress(
                data, {_CP.compressionLevel: lvl, _CP.nbWorkers: nt}
            )

        return mt

    for _label, _lvl in (("zstd-fast1", -1), ("zstd-3", 3), ("zstd-10", 10)):
        _add(
            Codec(
                _label,
                (lambda lv: lambda d: _pyzstd.compress(d, lv))(_lvl),
                lambda c, n: _pyzstd.decompress(c),
                mt_compress=_zstd_mt(_lvl),
            )
        )
except ImportError:  # pragma: no cover
    _add(_unavailable("zstd", "pip install pyzstd"))

# --- lzav / kanzi / zxc: optional native shims (see bench/native.py) ------
from . import native as _native  # noqa: E402

for _c in _native.codecs():
    _add(_c)


def _select(codec: Codec, numthreads: int) -> Codec:
    """Single-thread codec at ``numthreads <= 1``; the multithreaded variant (same
    name) when ``numthreads > 1`` and the codec has one. Never both."""
    if numthreads > 1 and codec.mt_compress is not None:
        mt = codec.mt_compress
        return Codec(
            codec.name,
            lambda d: mt(d, numthreads),
            codec.decompress,
            codec.available,
            codec.note,
        )
    return codec


def all_codecs(numthreads: int = 1) -> list[Codec]:
    return [_select(c, numthreads) for c in _REGISTRY]


def available_codecs(numthreads: int = 1) -> list[Codec]:
    return [c for c in all_codecs(numthreads) if c.available]
