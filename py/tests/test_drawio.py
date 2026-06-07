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


class _Proc:
    def __init__(self, returncode=0):
        self.returncode = returncode
        self.stdout = ""
        self.stderr = ""


def test_container_cli_prefers_podman(monkeypatch):
    monkeypatch.setattr(
        drawio.shutil, "which", lambda n: "/bin/podman" if n == "podman" else None
    )
    assert drawio._container_cli() == "/bin/podman"
    monkeypatch.setattr(drawio.shutil, "which", lambda n: None)
    assert drawio._container_cli() is None


def _wire(monkeypatch, tmp_path, *, cli="/bin/podman", image=True):
    """Point _WORK at a tmp dir and stub cli/image detection."""
    monkeypatch.setattr(drawio, "_WORK", tmp_path / "work")
    monkeypatch.setattr(drawio, "_container_cli", lambda: cli)
    monkeypatch.setattr(drawio, "_image_present", lambda c: image)


def test_render_via_container_no_cli_or_image(monkeypatch, tmp_path):
    src = tmp_path / "d.drawio"
    src.write_text("<x/>")
    _wire(monkeypatch, tmp_path, cli=None)
    assert drawio._render_via_container(src, tmp_path / "o.png", 2.0) is False
    _wire(monkeypatch, tmp_path, image=False)
    assert drawio._render_via_container(src, tmp_path / "o.png", 2.0) is False


def test_render_via_container_success(monkeypatch, tmp_path):
    src = tmp_path / "d.drawio"
    src.write_text("<x/>")
    _wire(monkeypatch, tmp_path)

    def fake_run(argv, **kw):
        (drawio._WORK / "diagram.png").write_bytes(b"\x89PNG-direct")
        return _Proc(0)

    monkeypatch.setattr(drawio.subprocess, "run", fake_run)
    out = tmp_path / "o.png"
    assert drawio._render_via_container(src, out, 2.0) is True
    assert out.read_bytes() == b"\x89PNG-direct"


def test_render_via_container_multipage_fallback(monkeypatch, tmp_path):
    src = tmp_path / "d.drawio"
    src.write_text("<x/>")
    _wire(monkeypatch, tmp_path)

    def fake_run(argv, **kw):
        (drawio._WORK / "diagram-1.png").write_bytes(b"\x89PNG-page")
        return _Proc(0)

    monkeypatch.setattr(drawio.subprocess, "run", fake_run)
    out = tmp_path / "o.png"
    assert drawio._render_via_container(src, out, 2.0) is True
    assert out.read_bytes() == b"\x89PNG-page"


def test_render_via_container_no_png_written(monkeypatch, tmp_path):
    src = tmp_path / "d.drawio"
    src.write_text("<x/>")
    _wire(monkeypatch, tmp_path)
    monkeypatch.setattr(drawio.subprocess, "run", lambda *a, **k: _Proc(0))
    assert drawio._render_via_container(src, tmp_path / "o.png", 2.0) is False


def test_render_via_container_run_fails(monkeypatch, tmp_path):
    src = tmp_path / "d.drawio"
    src.write_text("<x/>")
    _wire(monkeypatch, tmp_path)
    monkeypatch.setattr(drawio.subprocess, "run", lambda *a, **k: _Proc(1))
    assert drawio._render_via_container(src, tmp_path / "o.png", 2.0) is False


def test_render_with_desktop_invokes_cli(monkeypatch, tmp_path):
    calls = {}

    def fake_run(argv, **kw):
        calls["argv"] = argv
        return _Proc(0)

    monkeypatch.setattr(drawio.subprocess, "run", fake_run)
    drawio._render_with_desktop(
        "/usr/bin/drawio", tmp_path / "d.drawio", tmp_path / "o.png", 2.0
    )
    assert calls["argv"][0] == "/usr/bin/drawio"
    assert "--export" in calls["argv"]


_PODMAN = shutil.which("podman") or shutil.which("docker")
_HAVE_IMAGE = bool(_PODMAN) and drawio._image_present(_PODMAN)
_SAMPLE = drawio._REPO_ROOT / "data" / "testdata" / "drawio" / "C4.drawio"


@pytest.mark.slow
@pytest.mark.skipif(
    not _HAVE_IMAGE or not _SAMPLE.is_file(),
    reason="needs podman/docker + the drawio-export image (just drawio-setup)",
)
def test_real_container_render(tmp_path):
    out = drawio.render_drawio_to_png(_SAMPLE, tmp_path / "c4.png")
    assert Path(out).read_bytes().startswith(b"\x89PNG")
