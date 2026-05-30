"""sRGB <-> linear <-> OKLab conversions (vectorized, numpy).

OKLab (Björn Ottosson, 2020) is a perceptual color space; inverting its L axis
gives a perceptually even light<->dark flip while preserving hue and chroma.
All functions operate on float arrays with the last axis = channels.
"""

from __future__ import annotations

import numpy as np


def srgb_to_linear(c: np.ndarray) -> np.ndarray:
    c = np.asarray(c, dtype=np.float64)
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(c: np.ndarray) -> np.ndarray:
    c = np.asarray(c, dtype=np.float64)
    return np.where(
        c <= 0.0031308,
        12.92 * c,
        1.055 * np.power(np.clip(c, 0, None), 1 / 2.4) - 0.055,
    )


def linear_rgb_to_oklab(rgb: np.ndarray) -> np.ndarray:
    """`rgb` shape (..., 3) in linear light -> OKLab (..., 3)."""
    r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
    lc = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b
    mc = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b
    sc = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b
    l_ = np.cbrt(lc)
    m_ = np.cbrt(mc)
    s_ = np.cbrt(sc)
    big_l = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_
    a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_
    b2 = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_
    return np.stack([big_l, a, b2], axis=-1)


def oklab_to_linear_rgb(lab: np.ndarray) -> np.ndarray:
    """OKLab (..., 3) -> linear RGB (..., 3)."""
    big_l, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    l_ = big_l + 0.3963377774 * a + 0.2158037573 * b
    m_ = big_l - 0.1055613458 * a - 0.0638541728 * b
    s_ = big_l - 0.0894841775 * a - 1.2914855480 * b
    lc = l_**3
    mc = m_**3
    sc = s_**3
    r = 4.0767416621 * lc - 3.3077115913 * mc + 0.2309699292 * sc
    g = -1.2684380046 * lc + 2.6097574011 * mc - 0.3413193965 * sc
    b2 = -0.0041960863 * lc - 0.7034186147 * mc + 1.7076147010 * sc
    return np.stack([r, g, b2], axis=-1)


def invert_lightness_oklab(rgb_unit: np.ndarray) -> np.ndarray:
    """Invert OKLab L of sRGB colors in `[0,1]` (..., 3), preserving hue/chroma."""
    lab = linear_rgb_to_oklab(srgb_to_linear(rgb_unit))
    lab[..., 0] = 1.0 - lab[..., 0]
    out = linear_to_srgb(oklab_to_linear_rgb(lab))
    return np.clip(out, 0.0, 1.0)
