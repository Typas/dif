"""Registry of lossless compression codecs for the `.difr` benchmark.

Each :class:`Codec` self-detects availability at import time, so the harness
runs with whatever is installed and reports the rest as unavailable. Candidates
follow the project spec (plan.md). ``decompress`` receives the original length
because some libraries (libdeflate) require the output size up front.
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

    for _q in (5, 11):
        _add(
            Codec(
                f"brotli-{_q}",
                (lambda q: lambda d: _brotli.compress(d, quality=q))(_q),
                lambda c, n: _brotli.decompress(c),
            )
        )
except ImportError:  # pragma: no cover
    _add(_unavailable("brotli", "pip install brotli"))

# --- bzip3 ----------------------------------------------------------------
# bzip3 is block-based; lzbench maps a "level" to block_size via
# block_size = 1 << (19 + level), capped at 511 MiB. See
# https://github.com/inikep/lzbench/blob/6ab4808616e5e6163d3f4a898b7527a1940cc35e/bench/symmetric_codecs.cpp#L153
try:
    import bz3 as _bz3

    _BZ3_MAX = 511 << 20  # lzbench cap
    for _lvl in (1, 5):  # level 1 = 1 MiB, level 5 = 16 MiB block
        _bs = min(1 << (19 + _lvl), _BZ3_MAX)
        _mib = _bs >> 20
        _add(
            Codec(
                f"bzip3-{_lvl}",
                (lambda bs: lambda d: _bz3.compress(d, bs))(_bs),
                lambda c, n: _bz3.decompress(c),
                note=f"level={_lvl} block_size={_mib}MiB",
            )
        )
except ImportError:  # pragma: no cover
    _add(_unavailable("bzip3", "pip install bzip3"))

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
                f"lz4hc-{_lvl}",
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

# --- zstd (via pyzstd; fast = negative level) -----------------------------
try:
    import pyzstd as _pyzstd

    for _label, _lvl in (("zstd-fast1", -1), ("zstd-3", 3), ("zstd-10", 10), ("zstd-22", 22)):
        _add(
            Codec(
                _label,
                (lambda lv: lambda d: _pyzstd.compress(d, lv))(_lvl),
                lambda c, n: _pyzstd.decompress(c),
            )
        )
except ImportError:  # pragma: no cover
    _add(_unavailable("zstd", "pip install pyzstd"))

# --- lzav / kanzi: optional native shims (see bench/native.py) ------------
from . import native as _native  # noqa: E402

for _c in _native.codecs():
    _add(_c)


def all_codecs() -> list[Codec]:
    return list(_REGISTRY)


def available_codecs() -> list[Codec]:
    return [c for c in _REGISTRY if c.available]
