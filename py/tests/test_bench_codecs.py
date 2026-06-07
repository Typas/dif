"""Registry helpers in bench.codecs: selection, aliasing, mt wrapping."""

from __future__ import annotations

import pytest

from bench import codecs as cd


def test_unavailable_codec_marked_not_available():
    c = cd._unavailable("nope", "pip install nope")
    assert c.available is False and c.note == "pip install nope"


def test_select_wraps_mt_encoder_when_threads_gt_one():
    zstd = next(c for c in cd._REGISTRY if c.mt_compress is not None)
    mt = cd._select(zstd, 4)
    assert mt.name == zstd.name
    data = bytes(range(256)) * 64
    # The wrapped encoder runs the multithreaded path and still round-trips.
    assert mt.decompress(mt.compress(data), len(data)) == data
    # threads <= 1 returns the codec untouched.
    assert cd._select(zstd, 1) is zstd


def test_family_strips_level():
    assert cd._family("zstd-3") == "zstd"
    assert cd._family("store") == "store"


def test_canon_rewrites_aliases():
    assert cd._canon("bsc") == "libbsc"
    assert cd._canon("bsc-b25m0e1") == "libbsc-b25m0e1"
    assert cd._canon("zstd-3") == "zstd-3"


def test_dynamic_libbsc_only_for_libbsc_specs():
    assert cd._dynamic_libbsc("zstd-3") is None
    assert cd._dynamic_libbsc("libbsc") is None  # no `-` spec


def test_select_codecs_by_family_and_exact():
    fam = cd.select_codecs(["zstd"])
    assert fam and all(cd._family(c.name) == "zstd" for c in fam)
    exact = cd.select_codecs(["zstd-3"])
    assert [c.name for c in exact] == ["zstd-3"]


def test_select_codecs_none_returns_registry():
    assert cd.select_codecs(None) == cd.all_codecs()


def test_select_codecs_unknown_raises():
    with pytest.raises(ValueError, match="no registered codec matches"):
        cd.select_codecs(["definitely-not-a-codec"])
