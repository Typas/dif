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

/// Pixel count below which a per-pixel pass runs serially (thread-spawn overhead
/// dominates a small frame).
const PAR_MIN_PX: usize = 1 << 16;

/// Thread budget for a per-pixel pass over `px` pixels: hardware parallelism
/// capped at 16, and never more than one worker per [`PAR_MIN_PX`] band. Returns
/// 1 (serial) for small frames.
fn pass_threads(px: usize) -> usize {
    if px < PAR_MIN_PX {
        return 1;
    }
    let hw = std::thread::available_parallelism().map_or(1, |n| n.get());
    hw.clamp(1, 16).min(px / PAR_MIN_PX).max(1)
}

/// Fill `out` (length `w*h`, row-major) by computing each pixel independently:
/// `f(x, y, p)` returns the value for pixel `p = y*w + x`. With `threads >= 2` the
/// rows split into contiguous bands across scoped workers, each writing only its
/// own band, so the result is bit-identical to the serial loop regardless of how
/// many threads ran. Anything `f` reads is shared read-only.
pub(crate) fn fill_rows_n<T, F>(out: &mut [T], w: usize, h: usize, threads: usize, f: F)
where
    T: Send,
    F: Fn(usize, usize, usize) -> T + Sync,
{
    if threads < 2 || h < 2 || w == 0 {
        for (p, slot) in out.iter_mut().enumerate() {
            *slot = f(p % w, p / w, p);
        }
        return;
    }
    let rows_per = h.div_ceil(threads);
    let f = &f;
    std::thread::scope(|s| {
        for (band, chunk) in out.chunks_mut(rows_per * w).enumerate() {
            let y0 = band * rows_per;
            s.spawn(move || {
                for (i, slot) in chunk.iter_mut().enumerate() {
                    let (x, y) = (i % w, y0 + i / w);
                    *slot = f(x, y, y * w + x);
                }
            });
        }
    });
}

/// [`fill_rows_n`] with the automatic [`pass_threads`] budget for `w*h`. Used by
/// the region build's grow pass too, so both per-pixel passes band identically.
pub(crate) fn fill_rows<T, F>(out: &mut [T], w: usize, h: usize, f: F)
where
    T: Send,
    F: Fn(usize, usize, usize) -> T + Sync,
{
    fill_rows_n(out, w, h, pass_threads(w * h), f);
}

/// Combined OKLab edge energy from precomputed `L` and chroma `C` planes. Each
/// output pixel is an independent Sobel of read-only `l`/`c`, so the pass runs
/// over parallel row bands ([`fill_rows`]) for large frames.
pub fn edge_energy_planes(l: &[f64], c: &[f64], w: usize, h: usize) -> Vec<f64> {
    let mut e = vec![0.0; w * h];
    fill_rows(&mut e, w, h, |x, y, _p| {
        let (lx, ly) = sobel(l, w, h, x, y);
        let (cx, cy) = sobel(c, w, h, x, y);
        (lx * lx + ly * ly + W_C * (cx * cx + cy * cy)).sqrt()
    });
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

    // Banded parallel fill must be byte-identical to the serial fill, whatever the
    // thread count. Forcing threads here (not pass_threads) exercises the scoped
    // workers even on a single-core runner.
    #[test]
    fn fill_rows_parallel_matches_serial() {
        let (w, h) = (37usize, 41usize);
        let f = |x: usize, y: usize, p: usize| (x * 3 + y * 7 + p) as f64 * 0.5;
        let mut serial = vec![0.0; w * h];
        let mut parallel = vec![0.0; w * h];
        fill_rows_n(&mut serial, w, h, 1, f);
        fill_rows_n(&mut parallel, w, h, 5, f);
        assert_eq!(serial, parallel, "banded fill must equal serial");
    }

    // Small frames stay serial; a large one asks for a sane bounded thread count.
    #[test]
    fn pass_threads_serial_small_parallel_large() {
        assert_eq!(pass_threads(1024), 1);
        let t = pass_threads(8 * PAR_MIN_PX);
        assert!((1..=16).contains(&t), "threads {t}");
    }

    #[test]
    fn oklab_lc_planes_builds_from_rgba8() {
        let rgba = [0u8, 0, 0, 255, 255, 255, 255, 255];
        let (l, c) = oklab_lc_planes_rgba8(&rgba, 2, 1);
        assert!(l[0] < 0.1 && l[1] > 0.9, "L: {l:?}");
        assert!(c[0] < 0.01 && c[1] < 0.01, "chroma: {c:?}");
    }
}
