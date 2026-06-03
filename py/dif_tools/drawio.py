"""Render ``.drawio`` diagrams to PNG via a local container.

Primary path: the ``rlespinasse/drawio-export`` image (bundles drawio-desktop +
a headless browser via xvfb), run as a one-shot CLI under podman.  The diagram
is copied into a scratch dir mounted at ``/data``; the container writes the PNG
into ``/data/out``.  Fully local -- no calls out to diagrams.net.  Run
``just drawio-setup`` once to pull the image.

Fallback: a ``drawio``/``drawio-desktop`` CLI on ``PATH``.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
_WORK = Path("/tmp") / "drawio-work"

IMAGE = "docker.io/rlespinasse/drawio-export:v4.52.0"


def drawio_executable() -> str | None:
    """Return a drawio/drawio-desktop CLI on PATH, or ``None``."""
    for name in ("drawio", "drawio-desktop"):
        found = shutil.which(name)
        if found:
            return found
    return None


def _container_cli() -> str | None:
    return shutil.which("podman") or shutil.which("docker")


def _image_present(cli: str) -> bool:
    return (
        subprocess.run([cli, "image", "exists", IMAGE], capture_output=True).returncode
        == 0
    )


def _render_via_container(path: Path, out_png: Path, scale: float) -> bool:
    """Convert ``path`` to PNG inside the container; write to ``out_png``.

    Returns ``False`` (without raising) if podman/docker or the image is
    missing, so the caller can fall back.
    """
    cli = _container_cli()
    if cli is None or not _image_present(cli):
        return False

    # Fresh scratch dir mounted at /data; copy the source in under a stable name.
    if _WORK.exists():
        shutil.rmtree(_WORK)
    _WORK.mkdir(parents=True)
    src = _WORK / "diagram.drawio"
    src.write_bytes(path.read_bytes())

    proc = subprocess.run(
        [
            cli,
            "run",
            "--rm",
            "-v",
            f"{_WORK}:/data:z",
            IMAGE,
            "-f",
            "png",
            "-s",
            str(scale),
            "--remove-page-suffix",
            "-o",
            "/data",
            "-t",
            "diagram.drawio",
        ],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        return False

    produced = _WORK / "diagram.png"
    if not produced.is_file():
        # Multi-page or unexpected naming: take the first PNG written.
        pngs = sorted((_WORK).glob("*.png"))
        if not pngs:
            return False
        produced = pngs[0]

    out_png.write_bytes(produced.read_bytes())
    return True


def _render_with_desktop(exe: str, path: Path, out_png: Path, scale: float) -> None:
    subprocess.run(
        [
            exe,
            "--export",
            "--format",
            "png",
            "--scale",
            str(scale),
            "--output",
            str(out_png),
            str(path),
        ],
        check=True,
    )


def render_drawio_to_png(
    path: str | Path, out_png: str | Path | None = None, scale: float = 2.0
) -> str:
    """Export a ``.drawio`` file to PNG; returns the PNG path.

    Prefers the local container; falls back to a ``drawio-desktop`` CLI on
    ``PATH``.  Raises ``RuntimeError`` if neither is available.  The output
    defaults next to the source file.
    """
    path = Path(path)
    out_png = Path(out_png) if out_png is not None else path.with_suffix(".png")
    out_png.parent.mkdir(parents=True, exist_ok=True)

    exe = drawio_executable()
    if exe is not None:
        _render_with_desktop(exe, path, out_png, scale)
        return str(out_png)

    if _render_via_container(path, out_png, scale):
        return str(out_png)

    raise RuntimeError(
        "No drawio renderer available. Run `just drawio-setup` to pull the "
        "rlespinasse/drawio-export image (needs podman), or install "
        "drawio-desktop (https://github.com/jgraph/drawio-desktop)."
    )
