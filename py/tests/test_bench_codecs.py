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


def test_core_accepts_matches_dif_parse():
    assert cd._core_accepts("zstd--7") is True
    assert cd._core_accepts("lz4-hc10") is True
    assert cd._core_accepts("zstd-19") is False  # not in the core level table
    assert cd._core_accepts("nonsense") is False


def test_dynamic_zstd_builds_off_list_level():
    assert cd._dynamic_zstd("lz4-hc2") is None  # wrong family
    assert cd._dynamic_zstd("zstd-19") is None  # core won't store level 19
    c = cd._dynamic_zstd("zstd--7")
    assert c is not None and c.name == "zstd--7"
    data = bytes(range(256)) * 64
    assert c.decompress(c.compress(data), len(data)) == data


def test_dynamic_lz4_builds_fast_and_hc_levels():
    assert cd._dynamic_lz4("zstd-3") is None  # wrong family
    data = bytes(range(256)) * 64
    for tok in ("lz4-fast512", "lz4-hc10"):
        c = cd._dynamic_lz4(tok)
        assert c is not None and c.name == tok
        assert c.decompress(c.compress(data), len(data)) == data


def test_select_codecs_reaches_full_core_range():
    names = [c.name for c in cd.select_codecs(["zstd--7", "lz4-hc2", "lz4-fast512"])]
    assert names == ["zstd--7", "lz4-hc2", "lz4-fast512"]


def test_select_codecs_unstorable_level_raises():
    with pytest.raises(ValueError, match="no registered codec matches"):
        cd.select_codecs(["zstd-19"])  # valid family, level the core can't store
