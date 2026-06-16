//! Encode-side OKLab edge operators, gated behind the `encode` feature.
//!
//! Builds the OKLab lightness `L` and chroma `C` planes for a frame and the
//! combined Sobel edge energy `E = sqrt(gL^2 + w_c * gC^2)` over them. The
//! region-aware build ([`crate::build_regional`]) uses `E` to grow a text mask
//! into its anti-aliasing shell ([`crate::build_regional`] gates growth on
//! `E > TAU_EDGE`).

use alloc::vec;
use alloc::vec::Vec;

use palette::{IntoColor, Oklab, Srgb};

use crate::Rgba;

/// Weight of the chroma edge term relative to the lightness term in `E`.
const W_C: f64 = 1.0;
/// Edge-energy threshold: above this a pixel is a transition / real edge.
pub const TAU_EDGE: f64 = 0.06;

/// OKLab `[L, a, b]` of one palette color at color depth `max`.
fn oklab_of(c: Rgba, max: f64) -> [f64; 3] {
    let s = Srgb::new(c.r as f64 / max, c.g as f64 / max, c.b as f64 / max);
    let lab: Oklab<f64> = s.into_color();
    [lab.l, lab.a, lab.b]
}

/// Build OKLab lightness `L` and chroma planes from a packed RGBA8 buffer
/// (`4 * w * h` bytes; alpha ignored -- composite first). Exposed so the
/// theme-check QA harness measures edges with the same operator the codec uses.
pub fn oklab_lc_planes_rgba8(rgba: &[u8], w: usize, h: usize) -> (Vec<f64>, Vec<f64>) {
    let n = w * h;
    let mut l = vec![0.0; n];
    let mut c = vec![0.0; n];
    for i in 0..n {
        let o = i * 4;
        let px = Rgba::new(rgba[o] as u16, rgba[o + 1] as u16, rgba[o + 2] as u16, 255);
        let lab = oklab_of(px, 255.0);
        l[i] = lab[0];
        c[i] = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    }
    (l, c)
}

/// Build OKLab lightness `L` and chroma planes from an indexed image: convert
/// each palette color once, then scatter the result through the per-pixel id
/// plane `idx`. Bit-identical to [`oklab_lc_planes_rgba8`] on the same pixels
/// (alpha ignored the same way), but pays one sRGB->OKLab conversion per unique
/// color instead of one per pixel -- the region build's hot path, where a
/// diagram has thousands of pixels per color. `max` is the channel max for the
/// palette's depth.
pub fn oklab_lc_planes_indexed(idx: &[u32], palette: &[Rgba], max: f64) -> (Vec<f64>, Vec<f64>) {
    let table: Vec<(f64, f64)> = palette
        .iter()
        .map(|&c| {
            let lab = oklab_of(c, max);
            (lab[0], (lab[1] * lab[1] + lab[2] * lab[2]).sqrt())
        })
        .collect();
    let mut l = vec![0.0; idx.len()];
    let mut c = vec![0.0; idx.len()];
    for (p, &id) in idx.iter().enumerate() {
        let (li, ci) = table[id as usize];
        l[p] = li;
        c[p] = ci;
    }
    (l, c)
}

/// Sample `plane` at `(x, y)` with edge-replicate clamping.
#[inline]
fn at(plane: &[f64], w: usize, h: usize, x: isize, y: isize) -> f64 {
    let xc = x.clamp(0, w as isize - 1) as usize;
    let yc = y.clamp(0, h as isize - 1) as usize;
    plane[yc * w + xc]
}

/// 3x3 Sobel gradient `(gx, gy)` of `plane` at `(x, y)` (clamped borders).
fn sobel(plane: &[f64], w: usize, h: usize, x: usize, y: usize) -> (f64, f64) {
    let (x, y) = (x as isize, y as isize);
    let s = |dx: isize, dy: isize| at(plane, w, h, x + dx, y + dy);
    let gx = -s(-1, -1) - 2.0 * s(-1, 0) - s(-1, 1) + s(1, -1) + 2.0 * s(1, 0) + s(1, 1);
    let gy = -s(-1, -1) - 2.0 * s(0, -1) - s(1, -1) + s(-1, 1) + 2.0 * s(0, 1) + s(1, 1);
    (gx, gy)
}

/// Combined OKLab edge energy from precomputed `L` and chroma `C` planes.
pub fn edge_energy_planes(l: &[f64], c: &[f64], w: usize, h: usize) -> Vec<f64> {
    let mut e = vec![0.0; w * h];
    for (p, slot) in e.iter_mut().enumerate() {
        let (x, y) = (p % w, p / w);
        let (lx, ly) = sobel(l, w, h, x, y);
        let (cx, cy) = sobel(c, w, h, x, y);
        *slot = (lx * lx + ly * ly + W_C * (cx * cx + cy * cy)).sqrt();
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    // The chroma term must register an edge even when lightness is flat.
    #[test]
    fn edge_energy_uses_chroma() {
        let (w, h) = (4usize, 3usize);
        let l = vec![0.6; w * h]; // flat lightness everywhere
        let mut c = vec![0.0; w * h]; // chroma step at x=2
        for y in 0..h {
            for x in 2..w {
                c[y * w + x] = 0.2;
            }
        }
        let e = edge_energy_planes(&l, &c, w, h);
        assert!(
            e[1 * w + 1] > TAU_EDGE,
            "chroma-only edge energy {} below threshold",
            e[1 * w + 1]
        );
    }

    #[test]
    fn oklab_lc_planes_builds_from_rgba8() {
        let rgba = [0u8, 0, 0, 255, 255, 255, 255, 255];
        let (l, c) = oklab_lc_planes_rgba8(&rgba, 2, 1);
        assert!(l[0] < 0.1 && l[1] > 0.9, "L: {l:?}");
        assert!(c[0] < 0.01 && c[1] < 0.01, "chroma: {c:?}");
    }
}
