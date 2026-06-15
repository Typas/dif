"""Registry of lossless compression codecs for the `.difr` benchmark.

This benchmark measures *standalone* compression algorithms over the raw `.difr`
body --- it deliberately does **not** include the DIF container itself. Each
:class:`Codec` self-detects availability at import time, so the harness runs with
whatever is installed and reports the rest as unavailable. ``decompress`` receives
the original length because some libraries (libdeflate) require the output size up
front.

Codecs are thread-aware: a codec may carry an optional multithreaded encoder.
:func:`all_codecs` picks the multithreaded encoder when ``num_threads > 1`` (and the
codec has one), else the single-thread encoder --- never both for one codec.
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
    # Optional multithreaded encoder `(data, num_threads) -> bytes`; the stream
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

# --- brotli (5 = study default, 11 = max quality) -------------------------
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

    for _label, _lvl in (
        ("zstd-fast1", -1),
        ("zstd-3", 3),
        ("zstd-10", 10),
        ("zstd-18", 18),
        ("zstd-22", 22),
    ):
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

# --- lzav / kanzi / libbsc: optional native shims (see bench/native.py) ------
from . import native as _native  # noqa: E402

for _c in _native.codecs():
    _add(_c)


def _select(codec: Codec, num_threads: int) -> Codec:
    """Single-thread codec at ``num_threads <= 1``; the multithreaded variant (same
    name) when ``num_threads > 1`` and the codec has one. Never both."""
    if num_threads > 1 and codec.mt_compress is not None:
        mt = codec.mt_compress
        return Codec(
            codec.name,
            lambda d: mt(d, num_threads),
            codec.decompress,
            codec.available,
            codec.note,
        )
    return codec


def all_codecs(num_threads: int = 1) -> list[Codec]:
    return [_select(c, num_threads) for c in _REGISTRY]


def available_codecs(num_threads: int = 1) -> list[Codec]:
    return [c for c in all_codecs(num_threads) if c.available]


# Aliases so `--codecs` accepts the DIF family names (`bsc`, `deflate`) as well as
# the registry/table names (`libbsc`, `libdeflate`) the harness actually displays.
_FAMILY_ALIASES = {"bsc": "libbsc", "deflate": "libdeflate"}


def _family(name: str) -> str:
    """Codec family: the name without its trailing ``-level`` (``zstd-3`` -> ``zstd``)."""
    return name.rsplit("-", 1)[0] if "-" in name else name


def _canon(token: str) -> str:
    """Rewrite a DIF-family token to its registry name, keeping any ``-level``
    suffix (``bsc`` -> ``libbsc``, ``bsc-b25m0e1`` -> ``libbsc-b25m0e1``)."""
    fam = _family(token)
    return _FAMILY_ALIASES[fam] + token[len(fam) :] if fam in _FAMILY_ALIASES else token


def _dynamic_libbsc(token: str) -> Codec | None:
    """Build a libbsc codec for an on-demand ``b/m/e`` spec token that isn't a
    pre-registered default (e.g. ``libbsc-b1m3e2``).

    ``None`` when ``token`` isn't a ``libbsc-<b/m/e>`` spec or the shim isn't
    built; propagates ``ValueError`` (from :func:`native.make_libbsc`) when the
    token *is* a libbsc spec with an out-of-range field, so a typo errors clearly.
    """
    if _family(token) != "libbsc" or "-" not in token:
        return None
    return _native.make_libbsc(token.split("-", 1)[1])


def _core_accepts(token: str) -> bool:
    """True when dif-core's ``Codec::parse`` accepts ``token`` (the format can
    actually store this codec/level). The single source of truth for which zstd/lz4
    levels are benchable, so the standalone registry never drifts from the core."""
    try:
        import dif
    except ImportError:  # pragma: no cover - dif is always built for the bench
        return False
    try:
        dif.validate_codec(token)
        return True
    except ValueError:
        return False


def _dynamic_zstd(token: str) -> Codec | None:
    """Build a pyzstd codec for any core-valid zstd level not pre-registered
    (``zstd--7``, ``zstd-16``, ...). ``None`` when ``token`` isn't a core-accepted
    ``zstd-<level>`` spec or pyzstd is missing."""
    if not token.startswith("zstd-") or not _core_accepts(token):
        return None
    try:
        import pyzstd
    except ImportError:  # pragma: no cover - guarded by _core_accepts deps
        return None
    lvl = int(token[len("zstd-") :])
    return Codec(
        token,
        (lambda lv: lambda d: pyzstd.compress(d, lv))(lvl),
        lambda c, n: pyzstd.decompress(c),
        mt_compress=_zstd_mt(lvl),
    )


def _dynamic_lz4(token: str) -> Codec | None:
    """Build an lz4.block codec for any core-valid lz4 level not pre-registered
    (``lz4-fast512``, ``lz4-hc10``, ...). Core spells lz4 levels ``fast<n>`` (fast
    acceleration) / ``hc<n>`` (HC level). ``None`` when ``token`` isn't a
    core-accepted ``lz4-<level>`` spec or lz4 is missing."""
    if not token.startswith("lz4-") or not _core_accepts(token):
        return None
    try:
        import lz4.block as lz4b
    except ImportError:  # pragma: no cover - guarded by _core_accepts deps
        return None
    rest = token[len("lz4-") :]
    if rest.startswith("fast"):
        accel = int(rest[len("fast") :])
        comp = (
            lambda a: lambda d: lz4b.compress(
                d, mode="fast", acceleration=a, store_size=True
            )
        )(accel)
    else:  # hc<n>
        lvl = int(rest[len("hc") :])
        comp = (
            lambda lv: lambda d: lz4b.compress(
                d, mode="high_compression", compression=lv, store_size=True
            )
        )(lvl)
    return Codec(token, comp, lambda c, n: lz4b.decompress(c))


def select_codecs(specs: list[str] | None, num_threads: int = 1) -> list[Codec]:
    """Filter the registry by lzbench ``-e`` tokens (see ``bench.__main__._codecs``).

    A bare family token (``zstd``, ``libbsc``) selects every level of that family;
    a ``family-level`` token (``zstd-3``) selects that exact codec. Matching is
    against the names shown in the table, with the ``bsc``/``deflate`` aliases.
    A ``libbsc-<b/m/e>`` token (``bsc-b1m3e2``) not among the registered defaults
    is built on demand from the shim; likewise any ``zstd-<level>`` / ``lz4-<level>``
    the core accepts (``zstd--7``, ``lz4-hc10``) is built on demand, so the bench can
    reach every level the format can store, not just the pre-registered handful.
    ``None``/empty = the whole registry. Raises ``ValueError`` naming any token that
    matches no codec.
    """
    if not specs:
        return all_codecs(num_threads)
    tokens = [_canon(s) for s in specs]
    exact = {t for t in tokens if "-" in t}
    families = {t for t in tokens if "-" not in t}
    out = [
        c
        for c in all_codecs(num_threads)
        if c.name in exact or _family(c.name) in families
    ]
    matched = {c.name for c in out} | {_family(c.name) for c in out}
    missing: list[str] = []
    for orig, t in zip(specs, tokens):
        if t in matched:
            continue
        # on-demand: libbsc b/m/e spec, or any core-valid zstd/lz4 level
        c = _dynamic_libbsc(t) or _dynamic_zstd(t) or _dynamic_lz4(t)
        if c is not None:
            out.append(_select(c, num_threads))
            matched.add(t)
        else:
            missing.append(orig)
    if missing:
        names = ", ".join(sorted({c.name for c in _REGISTRY}))
        raise ValueError(
            f"no registered codec matches: {', '.join(missing)}; available: {names}"
        )
    return out
