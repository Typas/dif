//! Encode-side dark-theme derivation (OKLab), gated behind the `derive` feature.
//!
//! A single-theme source image synthesizes its alternate (dark) theme here. This
//! is the native port of the Python `dif_tools/colorspace.py` + `themes.py`
//! pipeline; the binding re-exports it so the converter never marshals a palette
//! across the FFI boundary. Uses the `palette` crate (f64) for the sRGB/OKLab/
//! OKLCh conversions, so results match the numpy reference within rounding.
//!
//! Three strategies (see [`Strategy`]):
//! - `Keep`       : dark theme identical to the source.
//! - `Invert`     : photographic negative (`max - channel`), alpha kept.
//! - `Arithmetic` : perceptual OKLab --- achromatic colors flip lightness
//!   (`L' = 1 - L`, white<->black) while chromatic colors keep hue and are
//!   tone-compressed into the dark band, then OKLCh-chroma gamut-mapped into sRGB
//!   so a light high-chroma color (e.g. yellow) stays a visible muted version of
//!   itself instead of crushing to near-black.
//!
//! Only the sRGB gamut is implemented (matching the Python default); P3/Rec.2020
//! remain WIP at the format level.

use alloc::vec::Vec;

use palette::{IntoColor, IsWithinBounds, Oklab, Oklch, Srgb};

use crate::{ColorDepth, DifError, Result, Rgba};

/// OKLab chroma below this counts as achromatic (gray): flip lightness instead
/// of tone-compressing. Mirrors `_ACHROMATIC_C` in the Python reference.
const ACHROMATIC_C: f64 = 1e-3;
/// Gamut-mapping chroma binary-search iterations (mirrors the Python loop).
const GAMUT_ITERS: usize = 25;

/// Dark-theme derivation strategy. Parsed from the binding's `&str`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strategy {
    Keep,
    Invert,
    Arithmetic,
}

impl Strategy {
    pub fn from_name(s: &str) -> Result<Self> {
        match s {
            "keep" => Ok(Strategy::Keep),
            "invert" => Ok(Strategy::Invert),
            "arithmetic" => Ok(Strategy::Arithmetic),
            _ => Err(DifError::Invalid(
                "strategy must be 'keep', 'invert', or 'arithmetic'",
            )),
        }
    }
}

/// Tone-compress lightness toward the dark band (no flip): a light color lands
/// mid-dark (visible), a dark color stays dark. Split at `L = 0.5`.
fn dark_lightness(l: f64) -> f64 {
    if l < 0.5 { l / 2.0 } else { l / 2.0 + 0.25 }
}

/// Arithmetic OKLab derivation of one color's RGB (alpha handled by the caller).
/// `r,g,b` are integer channel values in `0..=max`; returns dark `(r,g,b)`.
fn arithmetic_rgb(r: u16, g: u16, b: u16, max: f64) -> (u16, u16, u16) {
    let srgb = Srgb::new(r as f64 / max, g as f64 / max, b as f64 / max);
    let lab: Oklab<f64> = srgb.into_color();
    let chroma = (lab.a * lab.a + lab.b * lab.b).sqrt();
    let new_l = if chroma < ACHROMATIC_C {
        1.0 - lab.l
    } else {
        dark_lightness(lab.l)
    };
    let base: Oklch<f64> = Oklab::new(new_l, lab.a, lab.b).into_color();

    // Reduce OKLCh chroma until the color fits sRGB. The common case --- every
    // achromatic/gray color, plus most tone-compressed ones --- already fits, so
    // skip the search and keep full chroma (`k = 1`). Otherwise binary-search the
    // largest scale `k in [0,1]` that stays in gamut (shrinking toward (L, hue)).
    let in_gamut = |k: f64| -> bool {
        let cand = Oklch::new(base.l, base.chroma * k, base.hue);
        let s: Srgb<f64> = cand.into_color();
        s.is_within_bounds()
    };
    let k = if in_gamut(1.0) {
        1.0
    } else {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..GAMUT_ITERS {
            let mid = 0.5 * (lo + hi);
            if in_gamut(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    };

    let mapped: Srgb<f64> = Oklch::new(base.l, base.chroma * k, base.hue).into_color();
    let enc = |v: f64| (v.clamp(0.0, 1.0) * max).round() as u16;
    (enc(mapped.red), enc(mapped.green), enc(mapped.blue))
}

/// Map one color to its dark-theme color under `strategy`. Alpha is untouched.
fn dark_color(c: Rgba, strategy: Strategy, maxv: u16, max: f64) -> Rgba {
    match strategy {
        Strategy::Keep => c,
        Strategy::Invert => Rgba::new(maxv - c.r, maxv - c.g, maxv - c.b, c.a),
        Strategy::Arithmetic => {
            let (r, g, b) = arithmetic_rgb(c.r, c.g, c.b, max);
            Rgba::new(r, g, b, c.a)
        }
    }
}

/// Derive the dark-theme palette from a light (source) palette. The source theme
/// stays the lossless identity; this only builds the appended dark theme.
pub fn derive_dark_palette(palette: &[Rgba], strategy: Strategy, depth: ColorDepth) -> Vec<Rgba> {
    let maxv = depth.max_value();
    let max = maxv as f64;
    palette
        .iter()
        .map(|&c| dark_color(c, strategy, maxv, max))
        .collect()
}

/// Derive the dark theme's `base_color` (RGB8) from the source base color under
/// `strategy`, so the picker tie-breaks against a representative background.
pub fn derive_dark_base_color(base: [u8; 3], strategy: Strategy) -> [u8; 3] {
    let c = Rgba::new(base[0] as u16, base[1] as u16, base[2] as u16, 255);
    let d = dark_color(c, strategy, 255, 255.0);
    [d.r as u8, d.g as u8, d.b as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_palette_is_negative() {
        let pal = [
            Rgba::new(0, 0, 0, 255),
            Rgba::new(255, 255, 255, 255),
            Rgba::new(200, 30, 40, 128),
        ];
        let out = derive_dark_palette(&pal, Strategy::Invert, ColorDepth::Rgba8);
        assert_eq!(out[0], Rgba::new(255, 255, 255, 255)); // black -> white
        assert_eq!(out[1], Rgba::new(0, 0, 0, 255)); // white -> black
        assert_eq!(out[2], Rgba::new(55, 225, 215, 128)); // alpha preserved
    }

    #[test]
    fn arithmetic_lightness_inverts_extremes() {
        let pal = [Rgba::new(0, 0, 0, 255), Rgba::new(255, 255, 255, 255)];
        let out = derive_dark_palette(&pal, Strategy::Arithmetic, ColorDepth::Rgba8);
        let mean = |c: Rgba| (c.r as u32 + c.g as u32 + c.b as u32) as f64 / 3.0;
        assert!(mean(out[0]) > 200.0); // black -> light
        assert!(mean(out[1]) < 55.0); // white -> dark
    }

    #[test]
    fn arithmetic_chromatic_stays_visible() {
        // A light high-chroma yellow must stay a visible warm color, not near-black.
        let out = derive_dark_palette(
            &[Rgba::new(253, 216, 53, 200)],
            Strategy::Arithmetic,
            ColorDepth::Rgba8,
        )[0];
        assert!(out.r.max(out.g).max(out.b) > 120); // visible
        assert!(out.r > out.b && out.g > out.b); // still warm: R,G > B
        assert_eq!(out.a, 200); // alpha preserved
    }

    #[test]
    fn invert_base_color() {
        assert_eq!(
            derive_dark_base_color([255, 255, 255], Strategy::Invert),
            [0, 0, 0]
        );
        assert_eq!(
            derive_dark_base_color([0, 0, 0], Strategy::Invert),
            [255, 255, 255]
        );
    }

    #[test]
    fn strategy_from_name_parses_known_and_rejects_unknown() {
        assert_eq!(Strategy::from_name("keep"), Ok(Strategy::Keep));
        assert_eq!(Strategy::from_name("invert"), Ok(Strategy::Invert));
        assert_eq!(Strategy::from_name("arithmetic"), Ok(Strategy::Arithmetic));
        assert!(Strategy::from_name("bogus").is_err());
    }
}
