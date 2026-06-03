"""drawio renderer: dispatch logic (mocked) + a skip-able real container render."""

from __future__ import annotations

import shutil
from pathlib import Path

import pytest

from dif_tools import drawio


def test_drawio_executable_finds(monkeypatch):
    monkeypatch.setattr(drawio.shutil, "which", lambda name: "/usr/bin/drawio")
    assert drawio.drawio_executable() == "/usr/bin/drawio"


def test_drawio_executable_absent(monkeypatch):
    monkeypatch.setattr(drawio.shutil, "which", lambda name: None)
    assert drawio.drawio_executable() is None


def test_render_uses_desktop_when_on_path(monkeypatch, tmp_path):
    monkeypatch.setattr(drawio, "drawio_executable", lambda: "/usr/bin/drawio")

    def fake_desktop(exe, path, out_png, scale):
        out_png.write_bytes(b"\x89PNG")

    monkeypatch.setattr(drawio, "_render_with_desktop", fake_desktop)
    src = tmp_path / "d.drawio"
    src.write_text("<mxGraphModel/>")
    out = tmp_path / "d.png"

    assert drawio.render_drawio_to_png(src, out) == str(out)
    assert out.read_bytes().startswith(b"\x89PNG")


def test_render_falls_back_to_container(monkeypatch, tmp_path):
    monkeypatch.setattr(drawio, "drawio_executable", lambda: None)
    monkeypatch.setattr(drawio, "_render_via_container", lambda *a: True)
    src = tmp_path / "d.drawio"
    src.write_text("<mxGraphModel/>")
    out = tmp_path / "d.png"

    assert drawio.render_drawio_to_png(src, out) == str(out)


def test_render_raises_without_renderer(monkeypatch, tmp_path):
    monkeypatch.setattr(drawio, "drawio_executable", lambda: None)
    monkeypatch.setattr(drawio, "_render_via_container", lambda *a: False)
    src = tmp_path / "d.drawio"
    src.write_text("<mxGraphModel/>")

    with pytest.raises(RuntimeError, match="No drawio renderer"):
        drawio.render_drawio_to_png(src, tmp_path / "d.png")


_PODMAN = shutil.which("podman") or shutil.which("docker")
_HAVE_IMAGE = bool(_PODMAN) and drawio._image_present(_PODMAN)
_SAMPLE = drawio._REPO_ROOT / "testdata" / "drawio" / "C4.drawio"


@pytest.mark.slow
@pytest.mark.skipif(
    not _HAVE_IMAGE or not _SAMPLE.is_file(),
    reason="needs podman/docker + the drawio-export image (just drawio-setup)",
)
def test_real_container_render(tmp_path):
    out = drawio.render_drawio_to_png(_SAMPLE, tmp_path / "c4.png")
    assert Path(out).read_bytes().startswith(b"\x89PNG")
