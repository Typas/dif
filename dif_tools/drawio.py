"""Render ``.drawio`` diagrams to PNG via the drawio-desktop CLI.

drawio-desktop is optional. When it is absent, callers should pre-render the
diagram to PNG and feed that to the converter instead.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path


def drawio_executable() -> str | None:
    for name in ("drawio", "drawio-desktop"):
        found = shutil.which(name)
        if found:
            return found
    return None


def render_drawio_to_png(
    path: str | Path, out_png: str | Path | None = None, scale: float = 2.0
) -> str:
    """Export a ``.drawio`` file to PNG; returns the PNG path.

    Raises ``RuntimeError`` if the drawio CLI is not installed.
    """
    exe = drawio_executable()
    if exe is None:
        raise RuntimeError(
            "drawio CLI not found. Pre-render the diagram to PNG and pass that, "
            "or install drawio-desktop (https://github.com/jgraph/drawio-desktop)."
        )
    path = Path(path)
    out_png = Path(out_png) if out_png is not None else path.with_suffix(".png")
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
    return str(out_png)
