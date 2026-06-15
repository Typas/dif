"""Pure-Python paths in bench.native: spec parsing, probe, shim-absent guards."""

from __future__ import annotations

from pathlib import Path

import pytest

from bench import native
from bench.codecs import Codec


def _boom(*_a, **_k):
    raise RuntimeError("nope")


def test_make_libbsc_rejects_non_spec():
    assert native.make_libbsc("not-a-spec") is None


def test_make_libbsc_validates_fields():
    with pytest.raises(ValueError, match="block size"):
        native.make_libbsc("b9999m0e0")
    with pytest.raises(ValueError, match="block sorter"):
        native.make_libbsc("b25m9e0")
    with pytest.raises(ValueError, match="coder"):
        native.make_libbsc("b25m0e9")


def test_make_libbsc_returns_none_without_shim(monkeypatch):
    monkeypatch.setattr(native, "_libbsc_lib", None)
    monkeypatch.setattr(native, "_LIBBSC_SO", Path("/nonexistent/libbscshim.so"))
    # A valid spec (incl. a CUDA-only sorter) still yields None when unbuilt.
    assert native.make_libbsc("b25m0e1") is None
    assert native.make_libbsc("b25m7e0") is None


def test_libbsc_load_none_without_shim(monkeypatch):
    monkeypatch.setattr(native, "_libbsc_lib", None)
    monkeypatch.setattr(native, "_LIBBSC_SO", Path("/nonexistent/libbscshim.so"))
    assert native._libbsc_load() is None
    assert native._libbsc_codecs() == []


def test_codec_absent_branches(monkeypatch):
    monkeypatch.setattr(native, "_LZAV_SO", Path("/nonexistent/lzav.so"))
    monkeypatch.setattr(native, "_KANZI_SO", Path("/nonexistent/kanzi.so"))
    assert native._lzav_codecs() == []
    assert native._kanzi_codecs() == []


def test_unavailable_libbsc_flags_failures():
    cuda = Codec("libbsc-b25m7e0", _boom, _boom)
    plain = Codec("libbsc-b25m0e1", _boom, _boom)
    other = Codec("zstd-3", _boom, _boom)  # non-libbsc: skipped
    out = dict(native.unavailable_libbsc([cuda, plain, other]))
    assert "ST7" in out["libbsc-b25m7e0"] or "CUDA" in out["libbsc-b25m7e0"]
    assert out["libbsc-b25m0e1"] == "nope"  # raw shim error echoed
    assert "zstd-3" not in out


def test_libbsc_unavailable_reason_cuda_vs_raw():
    cuda = native._libbsc_unavailable_reason("libbsc-b25m8e0", RuntimeError("x"))
    assert "CUDA" in cuda and "ST8" in cuda
    raw = native._libbsc_unavailable_reason("libbsc-b25m0e0", RuntimeError("raw err"))
    assert raw == "raw err"
