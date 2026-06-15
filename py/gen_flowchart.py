"""Generate web/demo/flowchart.dif from scratch.

The wasm viewer and the docs gallery load `flowchart.dif` as the canonical demo
asset. This script *draws* it procedurally --- five rounded-free rectangular nodes
(red / yellow / green / green / blue) joined by gray elbow connectors on an
800x600 white canvas --- then encodes it through the shipped pipeline: the
region-aware dark theme is synthesized natively, and the body is written with the
shipped triplet (outer `store`, palette `zstd-16`, frame `zstd-10`).

This is the *source of truth* for the demo: there is no upstream image. Re-run via
`just regen-demo` after a container-format bump or a palette/derivation change ---
the output is deterministic, so a clean tree means nothing drifted.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np

from dif_tools import dif_image_from_array

_DIF = Path(__file__).resolve().parent.parent / "web" / "demo" / "flowchart.dif"

_W, _H = 800, 600
_BG = (255, 255, 255, 255)
_STROKE = (60, 60, 60, 255)  # node borders + connector lines
_BORDER = 4  # gray frame width, painted just outside each fill rect

# Nodes as (fill_x0, fill_y0, fill_x1, fill_y1, rgba) --- inclusive fill bounds.
# Each fill is 193x93; the gray frame is `_BORDER` px outside it on every edge.
_NODES = (
    (64, 44, 256, 136, (210, 38, 30, 255)),  # red    (top-left)
    (544, 44, 736, 136, (255, 214, 71, 255)),  # yellow (top-right)
    (64, 244, 256, 336, (76, 175, 80, 255)),  # green  (mid-left)
    (304, 444, 496, 536, (76, 175, 80, 255)),  # green  (bottom-center)
    (544, 444, 736, 536, (52, 120, 201, 255)),  # blue   (bottom-right)
)

# Gray connector segments as inclusive (x0, y0, x1, y1) rects (each 4 px thick).
_CONNECTORS = (
    (261, 89, 539, 92),  # red right  -> yellow left   (horizontal, at node mid-height)
    (399, 340, 402, 439),  # drop to green (bottom-center) top (vertical, free-standing)
    (159, 141, 162, 239),  # red bottom  -> green (mid-left) top   (vertical)
    (639, 141, 642, 439),  # yellow bottom -> blue top           (vertical)
)


def _fill(arr: np.ndarray, x0: int, y0: int, x1: int, y1: int, rgba) -> None:
    """Paint the inclusive rect [x0..x1] x [y0..y1] with `rgba`."""
    arr[y0 : y1 + 1, x0 : x1 + 1] = rgba


def _draw() -> np.ndarray:
    """Render the flowchart bitmap as an (H, W, 4) RGBA8 array."""
    arr = np.empty((_H, _W, 4), dtype=np.uint8)
    arr[:] = _BG
    for x0, y0, x1, y1, rgba in _NODES:
        _fill(arr, x0 - _BORDER, y0 - _BORDER, x1 + _BORDER, y1 + _BORDER, _STROKE)
        _fill(arr, x0, y0, x1, y1, rgba)
    for x0, y0, x1, y1 in _CONNECTORS:
        _fill(arr, x0, y0, x1, y1, _STROKE)
    return arr


def gen() -> None:
    arr = _draw()
    # Region-aware dark theme + shipped triplet, same path the converter ships.
    img = dif_image_from_array(arr, "arithmetic", "auto")
    blob = img.to_dif("store", "zstd-16", "zstd-10")

    import dif

    dif.Image.from_dif(blob)  # round-trip check before touching the asset
    _DIF.write_bytes(blob)
    print(f"{_DIF.name}: drawn -> v3 ({_W}x{_H}, {len(blob)} bytes)")


if __name__ == "__main__":
    gen()
