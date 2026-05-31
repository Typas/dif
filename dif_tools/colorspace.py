"""sRGB <-> linear <-> OKLab conversions (vectorized, numpy).

OKLab (Björn Ottosson, 2020) is a perceptual color space used to derive the
dark theme: achromatic colors flip lightness (`L' = 1 - L`, so a white
background goes black) while chromatic colors are tone-compressed (keeping hue)
so they stay recognizable instead of being crushed to near-black by a full
lightness inversion. All functions operate on float arrays, last axis = channels.
"""

from __future__ import annotations

import numpy as np

# Display gamuts the dark-theme mapping may target. A wider gamut needs less
# chroma reduction, so a P3/Rec.2020 display can keep more saturation than sRGB.
GAMUTS = ("srgb", "p3", "rec2020")


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


def _in_srgb(lab: np.ndarray, eps: float = 1e-4) -> np.ndarray:
    """Per-color mask: is this OKLab color inside the sRGB gamut?"""
    lin = oklab_to_linear_rgb(lab)
    return np.all(lin >= -eps, axis=-1) & np.all(lin <= 1.0 + eps, axis=-1)


def gamut_map_oklab(lab: np.ndarray, gamut: str = "srgb") -> np.ndarray:
    """Map OKLab colors into `gamut`, returning sRGB-encoded `[0,1]` (..., 3).

    Holds lightness and hue and reduces OKLCh chroma (scales `a,b` toward 0)
    until the color re-enters the gamut, then clips the tiny residual — so an
    out-of-gamut color becomes a *less saturated* version of itself instead of
    snapping to the nearest cube corner (pure black / white). This is what keeps
    an OKLab L-flip from crushing high-chroma light colors (e.g. yellow) to black.

    WARNING / WIP: only ``"srgb"`` is implemented. ``"p3"`` and ``"rec2020"``
    raise ``NotImplementedError`` — their wide-gamut boundary tests (OKLab ->
    P3/Rec.2020 linear) and output encoding are not done yet. See `GAMUTS`.
    """
    if gamut not in GAMUTS:
        raise ValueError(f"unknown gamut {gamut!r}; choose from {GAMUTS}")
    if gamut != "srgb":
        raise NotImplementedError(
            f"{gamut!r} gamut mapping is WIP — only 'srgb' is implemented"
        )

    lab = np.asarray(lab, dtype=np.float64)
    big_l, a, b = lab[..., 0], lab[..., 1], lab[..., 2]

    # Binary-search the largest chroma scale k in [0,1] that stays in sRGB.
    # In-gamut colors keep k=1 (no desaturation); out-of-gamut shrink toward L,H.
    inside = _in_srgb(lab)
    lo = np.where(inside, 1.0, 0.0)
    hi = np.ones_like(lo)
    for _ in range(25):
        mid = 0.5 * (lo + hi)
        ok = _in_srgb(np.stack([big_l, a * mid, b * mid], axis=-1))
        lo = np.where(ok, mid, lo)
        hi = np.where(ok, hi, mid)

    mapped = np.stack([big_l, a * lo, b * lo], axis=-1)
    out = linear_to_srgb(oklab_to_linear_rgb(mapped))
    return np.clip(out, 0.0, 1.0)


_ACHROMATIC_C = 1e-3  # OKLab chroma below this counts as gray (flip lightness).


def _dark_lightness(big_l: np.ndarray) -> np.ndarray:
    """Tone-compress lightness toward the dark band (no flip): a light color
    lands mid-dark (visible), a dark color stays dark. Split at L=0.5."""
    return np.where(big_l < 0.5, big_l / 2.0, big_l / 2.0 + 0.25)


def derive_dark_oklab(rgb_unit: np.ndarray, gamut: str = "srgb") -> np.ndarray:
    """Derive the dark-theme color for sRGB colors in `[0,1]` (..., 3).

    Achromatic colors (chroma ~ 0 — backgrounds, gridlines, text) flip fully
    (`L' = 1 - L`), so a white canvas becomes black. Chromatic colors keep their
    hue and are tone-compressed via `_dark_lightness` instead of inverted, so a
    light, high-chroma color (e.g. yellow) lands as a visible muted version of
    itself rather than being crushed to near-black. The result is gamut-mapped
    into `gamut` (see `gamut_map_oklab`).
    """
    lab = linear_rgb_to_oklab(srgb_to_linear(rgb_unit))
    big_l, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    chroma = np.hypot(a, b)
    lab[..., 0] = np.where(chroma < _ACHROMATIC_C, 1.0 - big_l, _dark_lightness(big_l))
    return gamut_map_oklab(lab, gamut)
