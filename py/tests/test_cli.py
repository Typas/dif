"""CLI entrypoints: `python -m dif_tools` and `python -m bench` via main(argv)."""

from __future__ import annotations

import argparse

import numpy as np
import pytest
from PIL import Image as PILImage

from bench import native
from bench.__main__ import _codecs, _images, _index_widths
from bench.__main__ import main as bench_main
from dif_tools.__main__ import main as dif_main


def _toy_png(path):
    arr = np.zeros((16, 16, 4), np.uint8)
    arr[..., 3] = 255
    arr[:8, :8, :3] = (200, 30, 40)
    PILImage.fromarray(arr, "RGBA").save(path)
    return path


def test_dif_convert_cli_writes_dif(tmp_path, capsys):
    src = _toy_png(tmp_path / "in.png")
    out = tmp_path / "out.dif"
    rc = dif_main(["convert", str(src), str(out), "--codec", "zstd-3"])
    assert rc == 0
    assert out.read_bytes()[:4] == b"DIF3"
    assert "wrote" in capsys.readouterr().out


def test_dif_convert_cli_raw(tmp_path):
    src = _toy_png(tmp_path / "in.png")
    out = tmp_path / "out.difr"
    assert dif_main(["convert", str(src), str(out), "--raw"]) == 0
    assert out.read_bytes()[:5] == b"DIFR3"


def test_bench_images_expands_dir(tmp_path):
    _toy_png(tmp_path / "a.png")
    _toy_png(tmp_path / "b.png")
    (tmp_path / "notes.txt").write_text("ignored")
    got = _images([str(tmp_path)])
    assert [p.rsplit("/", 1)[-1] for p in got] == ["a.png", "b.png"]


def test_bench_no_images_errors(capsys):
    assert bench_main(["codecs"]) == 1
    assert "no images" in capsys.readouterr().out


def test_bench_codecs_cli(tmp_path):
    src = _toy_png(tmp_path / "d.png")
    out = tmp_path / "c.tsv"
    report = tmp_path / "c.md"
    rc = bench_main(
        [
            "codecs",
            str(src),
            "--repeats",
            "1",
            "--out",
            str(out),
            "--report",
            str(report),
        ]
    )
    assert rc == 0
    assert out.read_text().startswith("image\t")  # TSV header
    assert report.read_text().strip()


def test_bench_formats_cli(tmp_path):
    src = _toy_png(tmp_path / "d.png")
    out = tmp_path / "f.tsv"
    report = tmp_path / "f.md"
    rc = bench_main(
        [
            "formats",
            str(src),
            "--repeats",
            "1",
            "--outer-codecs",
            "zstd-3",
            "--out",
            str(out),
            "--report",
            str(report),
        ]
    )
    assert rc == 0
    assert out.read_text().startswith("image\t")
    assert report.read_text().strip()


def test_bench_formats_dif_only_cli(tmp_path):
    src = _toy_png(tmp_path / "d.png")
    out = tmp_path / "f.tsv"
    report = tmp_path / "f.md"
    rc = bench_main(
        [
            "formats",
            str(src),
            "--dif-only",
            "--repeats",
            "1",
            "--outer-codecs",
            "zstd-3",
            "--out",
            str(out),
            "--report",
            str(report),
        ]
    )
    assert rc == 0
    lines = out.read_text().splitlines()
    assert lines[0].startswith("image\t")
    formats = [line.split("\t")[1] for line in lines[1:] if line]
    assert "png" not in formats


def test_codecs_spec_parses_levels_and_skips_empty():
    # `,` enumerates levels; a trailing `/` yields an empty segment that is skipped.
    assert _codecs("a/b,1,2/") == ["a", "b-1", "b-2"]


def test_index_widths_validate():
    assert _index_widths("auto/8/16") == ["auto", "8", "16"]
    with pytest.raises(argparse.ArgumentTypeError, match="invalid index width"):
        _index_widths("auto/7")


def test_setup_reports_all_built(monkeypatch, capsys):
    monkeypatch.setattr(native, "build_lzav", lambda: True)
    monkeypatch.setattr(native, "build_kanzi", lambda: True)
    monkeypatch.setattr(native, "build_libbsc", lambda cuda=False: True)
    assert bench_main(["setup"]) == 0
    out = capsys.readouterr().out
    assert "lzav shim: built" in out and "libbsc shim: built" in out


def test_setup_cuda_reports_failures(monkeypatch, capsys):
    monkeypatch.setattr(native, "build_lzav", lambda: False)
    monkeypatch.setattr(native, "build_kanzi", lambda: False)
    monkeypatch.setattr(native, "build_libbsc", lambda cuda=False: False)
    assert bench_main(["setup", "--cuda"]) == 0
    out = capsys.readouterr().out
    assert "FAILED" in out and "nvcc" in out


def test_codecs_unknown_codec_errors(tmp_path):
    src = _toy_png(tmp_path / "d.png")
    with pytest.raises(SystemExit):
        bench_main(["codecs", str(src), "--codecs", "definitely-not-a-codec"])


def test_codecs_unavailable_libbsc_refused(tmp_path, monkeypatch, capsys):
    src = _toy_png(tmp_path / "d.png")
    monkeypatch.setattr(
        native, "unavailable_libbsc", lambda sel: [("libbsc-b25m7e0", "needs CUDA")]
    )
    rc = bench_main(["codecs", str(src), "--repeats", "1"])
    assert rc == 1
    assert "not available" in capsys.readouterr().out


def test_formats_unknown_codec_errors(tmp_path):
    src = _toy_png(tmp_path / "d.png")
    with pytest.raises(SystemExit):
        bench_main(["formats", str(src), "--outer-codecs", "bogus"])


def test_bench_lfs_pointer_detected(tmp_path, capsys):
    ptr = tmp_path / "fake.png"
    ptr.write_bytes(
        b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 1234\n"
    )
    rc = bench_main(["formats", str(ptr)])
    assert rc == 1
    out = capsys.readouterr().out
    assert "LFS" in out and "git lfs pull" in out
