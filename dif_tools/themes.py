"""Theme-generation strategies.

A single-theme source image must synthesize its alternate (dark) theme. Three
strategies, matching the project spec:

- ``keep``       : alternate theme identical to the source (theme-agnostic).
- ``invert``     : photographic negative — ``max - value`` per channel
                   (grayscale: ``max - sample``). Cheap "revert the grayscale".
- ``arithmetic`` : perceptual OKLab lightness inversion across the whole
                   palette, preserving hue/chroma (grayscale: per-level OKLab).

Every strategy keeps alpha untouched and the *source* theme as the lossless
identity, so decoding the source theme reproduces the original pixels exactly.
"""

from __future__ import annotations

import numpy as np

from .colorspace import invert_lightness_oklab

STRATEGIES = ("keep", "invert", "arithmetic")


def derive_palette(colors: np.ndarray, strategy: str, max_value: int) -> np.ndarray:
    """Map an ``(N, 4)`` RGBA palette (ints ``0..max_value``) to the dark theme."""
    colors = np.asarray(colors)
    if strategy == "keep":
        return colors.copy()
    rgb = colors[:, :3]
    alpha = colors[:, 3:4]
    if strategy == "invert":
        new_rgb = max_value - rgb
    elif strategy == "arithmetic":
        unit = rgb.astype(np.float64) / max_value
        new_rgb = np.rint(invert_lightness_oklab(unit) * max_value)
    else:
        raise ValueError(f"unknown strategy {strategy!r}; choose from {STRATEGIES}")
    new_rgb = np.clip(new_rgb, 0, max_value).astype(colors.dtype)
    return np.concatenate([new_rgb, alpha], axis=1)


def derive_lut(strategy: str, max_value: int) -> list[int]:
    """Build the dark-theme grayscale LUT over ``0..=max_value``."""
    levels = max_value + 1
    base = np.arange(levels, dtype=np.int64)
    if strategy == "keep":
        return base.tolist()
    if strategy == "invert":
        return (max_value - base).tolist()
    if strategy == "arithmetic":
        gray = np.repeat((base / max_value)[:, None], 3, axis=1)  # (levels, 3)
        out = invert_lightness_oklab(gray)[:, 0]  # gray stays gray; take one channel
        return np.clip(np.rint(out * max_value), 0, max_value).astype(np.int64).tolist()
    raise ValueError(f"unknown strategy {strategy!r}; choose from {STRATEGIES}")


def identity_lut(max_value: int) -> list[int]:
    return list(range(max_value + 1))
