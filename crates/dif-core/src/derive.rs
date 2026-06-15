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
/// Foreground dark-lightness contrast lift (light -> dark derivation): push the
/// derived lightness of foreground-like regions (e.g. colored text) apart from
/// the dark background so they stay legible. `clamp(L * 1.1)` per the design.
const FG_CONTRAST_K: f64 = 1.1;

/// Which kind of region a color belongs to, deciding its dark-theme transform.
/// `Background` is the historical behavior; `Foreground` is slightly higher
/// contrast. Spatial classification (which pixels are which) lives in
/// `regions`/`aa_detect`; this enum only parameterizes the per-color math.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegionClass {
    Background,
    Foreground,
}

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

/// Arithmetic OKLab derivation of one color's RGB (alpha handled by the caller),
/// parameterized by region class. `r,g,b` are integer channel values in
/// `0..=max`; returns dark `(r,g,b)`. `Background` is byte-identical to the
/// historical transform; `Foreground` lifts the derived lightness for contrast.
fn arithmetic_rgb_region(r: u16, g: u16, b: u16, max: f64, class: RegionClass) -> (u16, u16, u16) {
    let srgb = Srgb::new(r as f64 / max, g as f64 / max, b as f64 / max);
    let lab: Oklab<f64> = srgb.into_color();
    let chroma = (lab.a * lab.a + lab.b * lab.b).sqrt();
    let mut new_l = if chroma < ACHROMATIC_C {
        1.0 - lab.l
    } else {
        dark_lightness(lab.l)
    };
    if matches!(class, RegionClass::Foreground) {
        new_l = (new_l * FG_CONTRAST_K).clamp(0.0, 1.0);
    }
    gamut_clamp_oklab(Oklab::new(new_l, lab.a, lab.b), max)
}

/// Map an OKLab color into sRGB, reducing OKLCh chroma toward the `(L, hue)` axis
/// until it fits the gamut, then encode to integer channels in `0..=max`. The
/// common case (achromatic / tone-compressed colors) already fits, so the search
/// is skipped; otherwise a fixed binary search finds the largest in-gamut chroma
/// scale. Shared by the arithmetic derivation and the structural optimizer so
/// both clamp identically.
pub(crate) fn gamut_clamp_oklab(lab: Oklab<f64>, max: f64) -> (u16, u16, u16) {
    let base: Oklch<f64> = lab.into_color();
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

/// Map one color to its dark-theme color under `strategy` and region `class`.
/// Alpha is untouched. `RegionClass::Background` reproduces the historical math.
fn dark_color_region(c: Rgba, strategy: Strategy, class: RegionClass, maxv: u16, max: f64) -> Rgba {
    match strategy {
        Strategy::Keep => c,
        Strategy::Invert => Rgba::new(maxv - c.r, maxv - c.g, maxv - c.b, c.a),
        Strategy::Arithmetic => {
            let (r, g, b) = arithmetic_rgb_region(c.r, c.g, c.b, max, class);
            Rgba::new(r, g, b, c.a)
        }
    }
}

/// Map one color to its dark-theme color under `strategy` (background region).
fn dark_color(c: Rgba, strategy: Strategy, maxv: u16, max: f64) -> Rgba {
    dark_color_region(c, strategy, RegionClass::Background, maxv, max)
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

/// Derive one color's dark-theme color under `strategy` and region `class` at
/// color depth `depth`. Public entry used by the region-aware split path.
pub fn dark_color_for(c: Rgba, strategy: Strategy, class: RegionClass, depth: ColorDepth) -> Rgba {
    let maxv = depth.max_value();
    dark_color_region(c, strategy, class, maxv, maxv as f64)
}

/// Dark color for a **text** pixel: a continuous lightness inversion `L' = 1 - L`
/// keeping hue, with no chroma branch. Applied to every pixel of a text region
/// (glyph core AND its anti-aliased fringe) so the inverted glyph stays a smooth
/// shape -- the chroma-threshold branch in [`dark_color_for`] is what splits a
/// glyph's fringe into flip-to-white and compress-to-dark pixels (the mottled,
/// un-OCR-able text). Alpha untouched.
pub fn text_dark_for(c: Rgba, depth: ColorDepth) -> Rgba {
    let max = depth.max_value() as f64;
    let lab = to_oklab(c, max);
    let (r, g, b) = gamut_clamp_oklab(Oklab::new(1.0 - lab.l, lab.a, lab.b), max);
    Rgba::new(r, g, b, c.a)
}

/// Convert one color to OKLab; `max` is the channel max (255 / 65535). Shared
/// with the structural optimizer.
pub(crate) fn to_oklab(c: Rgba, max: f64) -> Oklab<f64> {
    Srgb::new(c.r as f64 / max, c.g as f64 / max, c.b as f64 / max).into_color()
}

/// True when two colors are within `eps` Euclidean OKLab distance at `depth`.
pub fn oklab_close(a: Rgba, b: Rgba, depth: ColorDepth, eps: f64) -> bool {
    if a == b {
        return true;
    }
    let max = depth.max_value() as f64;
    let (la, lb) = (to_oklab(a, max), to_oklab(b, max));
    let d = (la.l - lb.l).powi(2) + (la.a - lb.a).powi(2) + (la.b - lb.b).powi(2);
    d.sqrt() <= eps
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
    fn text_dark_for_inverts_lightness_keeps_alpha() {
        // black text -> near white; white -> near black; alpha preserved.
        let blk = text_dark_for(Rgba::new(0, 0, 0, 255), ColorDepth::Rgba8);
        assert!(
            blk.r > 240 && blk.g > 240 && blk.b > 240,
            "black->white {blk:?}"
        );
        let wht = text_dark_for(Rgba::new(255, 255, 255, 255), ColorDepth::Rgba8);
        assert!(
            wht.r < 15 && wht.g < 15 && wht.b < 15,
            "white->black {wht:?}"
        );
        assert_eq!(
            text_dark_for(Rgba::new(0, 0, 0, 128), ColorDepth::Rgba8).a,
            128
        );
        // a near-black AA fringe (slightly chromatic) also inverts to bright --
        // no chroma branch, so it tracks the glyph instead of staying dark.
        let fringe = text_dark_for(Rgba::new(3, 3, 2, 255), ColorDepth::Rgba8);
        let lum = 0.299 * fringe.r as f64 + 0.587 * fringe.g as f64 + 0.114 * fringe.b as f64;
        assert!(
            lum > 200.0,
            "near-black fringe should inverts bright, lum {lum}"
        );
    }

    #[test]
    fn oklab_close_detects_near_and_far() {
        let a = Rgba::new(100, 100, 100, 255);
        assert!(oklab_close(a, a, ColorDepth::Rgba8, 0.0));
        assert!(oklab_close(
            a,
            Rgba::new(101, 100, 100, 255),
            ColorDepth::Rgba8,
            0.02
        ));
        assert!(!oklab_close(
            a,
            Rgba::new(0, 0, 0, 255),
            ColorDepth::Rgba8,
            0.02
        ));
    }

    #[test]
    fn dark_color_for_foreground_lifts() {
        let c = Rgba::new(120, 120, 120, 255);
        let bg = dark_color_for(
            c,
            Strategy::Arithmetic,
            RegionClass::Background,
            ColorDepth::Rgba8,
        );
        let fg = dark_color_for(
            c,
            Strategy::Arithmetic,
            RegionClass::Foreground,
            ColorDepth::Rgba8,
        );
        let mean = |x: Rgba| (x.r as u32 + x.g as u32 + x.b as u32) / 3;
        assert!(mean(fg) > mean(bg));
    }

    #[test]
    fn strategy_from_name_parses_known_and_rejects_unknown() {
        assert_eq!(Strategy::from_name("keep"), Ok(Strategy::Keep));
        assert_eq!(Strategy::from_name("invert"), Ok(Strategy::Invert));
        assert_eq!(Strategy::from_name("arithmetic"), Ok(Strategy::Arithmetic));
        assert!(Strategy::from_name("bogus").is_err());
    }
}
