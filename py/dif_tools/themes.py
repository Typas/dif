"""Theme-generation strategies.

A single-theme source image must synthesize its alternate (dark) theme. Three
strategies, matching the project spec:

- ``keep``       : alternate theme identical to the source (theme-agnostic).
- ``invert``     : photographic negative --- ``max - value`` per channel.
- ``arithmetic`` : perceptual OKLab dark-theme derivation --- achromatic colors
                   flip lightness (white<->black) while chromatic colors keep
                   hue and are tone-compressed into the dark band (so a light
                   color like yellow stays a visible muted color, not black),
                   then gamut-mapped.

Every strategy keeps alpha untouched and the *source* theme as the lossless
identity, so decoding the source theme reproduces the original pixels exactly.

The derivation itself lives in Rust (``dif`` extension, OKLab via the ``palette``
crate); these helpers are thin wrappers so Python callers/tests share that single
implementation. The converter doesn't use them --- it calls ``Image.add_dark_theme``
so no palette crosses the FFI boundary.
"""

from __future__ import annotations

from typing import cast

import dif
import numpy as np

STRATEGIES = ("keep", "invert", "arithmetic")


def derive_palette(colors: np.ndarray, strategy: str, max_value: int) -> np.ndarray:
    """Map an ``(N, 4)`` RGBA palette (ints ``0..max_value``) to the dark theme."""
    colors = np.asarray(colors)
    pal = [(int(c[0]), int(c[1]), int(c[2]), int(c[3])) for c in colors]
    dark = dif.derive_dark_palette(pal, cast("dif.Strategy", strategy), max_value)
    return np.asarray(dark, dtype=colors.dtype)


def derive_base_color(
    base: tuple[int, int, int], strategy: str
) -> tuple[int, int, int]:
    """Derive the dark theme's base color (RGB8) from a light base color."""
    return dif.derive_dark_base_color(base, cast("dif.Strategy", strategy))
