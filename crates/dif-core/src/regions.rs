//! Encode-side region labeling, gated behind the `encode` feature.
//!
//! Classifies every pixel of an indexed frame as foreground-like or
//! background-like so the dark-theme derivation can apply a higher-contrast
//! transform to small, thin, high-contrast regions (e.g. colored text, icon
//! strokes) while leaving large fills and the canvas on the historical path.
//!
//! The signal is purely structural and runs on the **index plane** the builder
//! already produced (no float image needed for the topology): connected
//! components of equal index, then per-component features
//! (`area`, `thinness = area/perimeter`, OKLab `chroma`, OKLab `dL` contrast
//! against the dominant bordering color). A small scored vote labels each
//! component; the label is then written to all of its pixels.
//!
//! Determinism: components are keyed by their representative pixel order and the
//! dominant-neighbor tie is broken by the smaller index, so the per-pixel label
//! plane is reproducible.

use alloc::vec;
use alloc::vec::Vec;

use palette::{IntoColor, Oklab, Srgb};
use rustc_hash::FxHashMap;

use crate::{ColorDepth, RegionClass, Rgba};

/// Area fraction below which a component leans foreground (small shapes/text).
const A_SMALL: f64 = 0.02;
/// `area / perimeter` below which a component is a thin stroke (text, outlines).
const T_THIN: f64 = 2.5;
/// OKLab lightness contrast against the dominant bordering color, above which a
/// component leans foreground (text stands out from its fill).
const C_HI: f64 = 0.25;
/// OKLab chroma above which a component leans foreground (colored ink/icons).
const CH_HI: f64 = 0.08;
/// Votes needed (out of four features) to label a component `Foreground`.
const FG_VOTES: u32 = 2;

/// OKLab lightness and chroma of one palette color at color depth `max`.
fn oklab_l_chroma(c: Rgba, max: f64) -> (f64, f64) {
    let srgb = Srgb::new(c.r as f64 / max, c.g as f64 / max, c.b as f64 / max);
    let lab: Oklab<f64> = srgb.into_color();
    (lab.l, (lab.a * lab.a + lab.b * lab.b).sqrt())
}

/// Union-find root with path halving.
fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        parent[x as usize] = parent[parent[x as usize] as usize];
        x = parent[x as usize];
    }
    x
}

fn union(parent: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        // Attach the larger root index under the smaller for a stable forest.
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parent[hi as usize] = lo;
    }
}

/// Per-component accumulators, indexed by the component's union-find root.
#[derive(Default, Clone)]
struct Comp {
    area: u32,
    perimeter: u32,
    index: u32,
    /// Dominant bordering color: best (count, index) seen so far.
    best_nbr_count: u32,
    best_nbr_index: u32,
    has_nbr: bool,
}

/// Classify every pixel of an indexed `width * height` frame into a
/// [`RegionClass`]. `indices` is row-major palette indices (length
/// `width * height`); `palette` maps an index to its color. Returns a per-pixel
/// label plane parallel to `indices`.
pub fn classify_regions(
    width: usize,
    height: usize,
    indices: &[u32],
    palette: &[Rgba],
    depth: ColorDepth,
) -> Vec<RegionClass> {
    let n = width * height;
    debug_assert_eq!(indices.len(), n);
    if n == 0 {
        return Vec::new();
    }

    // Precompute per-palette OKLab L + chroma for feature lookups.
    let max = depth.max_value() as f64;
    let pal_lc: Vec<(f64, f64)> = palette.iter().map(|&c| oklab_l_chroma(c, max)).collect();
    let lookup_l = |idx: u32| pal_lc.get(idx as usize).map(|&(l, _)| l).unwrap_or(0.0);
    let lookup_chroma = |idx: u32| pal_lc.get(idx as usize).map(|&(_, c)| c).unwrap_or(0.0);

    // Connected components (4-connectivity) of equal index.
    let mut parent: Vec<u32> = (0..n as u32).collect();
    for y in 0..height {
        for x in 0..width {
            let p = y * width + x;
            let idx = indices[p];
            if x + 1 < width && indices[p + 1] == idx {
                union(&mut parent, p as u32, (p + 1) as u32);
            }
            if y + 1 < height && indices[p + width] == idx {
                union(&mut parent, p as u32, (p + width) as u32);
            }
        }
    }

    // Accumulate component features. perimeter counts 4-neighbor edges that face
    // the image border or a different index; the dominant differing neighbor
    // index (count desc, index asc tie-break) feeds the contrast feature.
    let mut comps: Vec<Comp> = vec![Comp::default(); n];

    // Area + index + perimeter per component. Neighbor counts need a full tally
    // to pick the dominant differing index deterministically, so key a compact
    // map by (root, neighbor_index).
    let mut nbr_counts: FxHashMap<(u32, u32), u32> = FxHashMap::default();
    for y in 0..height {
        for x in 0..width {
            let p = y * width + x;
            let root = find(&mut parent, p as u32);
            let idx = indices[p];
            let c = &mut comps[root as usize];
            c.area += 1;
            c.index = idx;
            // 4-neighbors; out-of-bounds counts toward perimeter.
            let mut edge = |nbr: Option<usize>| match nbr {
                None => true, // border edge
                Some(q) => {
                    let nidx = indices[q];
                    if nidx != idx {
                        *nbr_counts.entry((root, nidx)).or_insert(0) += 1;
                        true
                    } else {
                        false
                    }
                }
            };
            let left = if x > 0 { Some(p - 1) } else { None };
            let right = if x + 1 < width { Some(p + 1) } else { None };
            let up = if y > 0 { Some(p - width) } else { None };
            let down = if y + 1 < height {
                Some(p + width)
            } else {
                None
            };
            for e in [left, right, up, down] {
                if edge(e) {
                    comps[root as usize].perimeter += 1;
                }
            }
        }
    }

    // Reduce the neighbor tally to each component's dominant differing index.
    for (&(root, nidx), &count) in &nbr_counts {
        let c = &mut comps[root as usize];
        if !c.has_nbr
            || count > c.best_nbr_count
            || (count == c.best_nbr_count && nidx < c.best_nbr_index)
        {
            c.has_nbr = true;
            c.best_nbr_count = count;
            c.best_nbr_index = nidx;
        }
    }

    // Score each component and write the label to every pixel.
    let total = n as f64;
    let mut out = vec![RegionClass::Background; n];
    for (p, slot) in out.iter_mut().enumerate() {
        let root = find(&mut parent, p as u32) as usize;
        let c = &comps[root];
        let area_fraction = c.area as f64 / total;
        let thinness = if c.perimeter > 0 {
            c.area as f64 / c.perimeter as f64
        } else {
            f64::INFINITY
        };
        let chroma = lookup_chroma(c.index);
        let contrast = if c.has_nbr {
            (lookup_l(c.index) - lookup_l(c.best_nbr_index)).abs()
        } else {
            0.0
        };
        let votes = (area_fraction < A_SMALL) as u32
            + (thinness < T_THIN) as u32
            + (contrast > C_HI) as u32
            + (chroma > CH_HI) as u32;
        *slot = if votes >= FG_VOTES {
            RegionClass::Foreground
        } else {
            RegionClass::Background
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A large index-0 fill with a short, thin index-1 stroke floating in the
    // middle (not touching any edge, so the fill stays one big connected region).
    // The stroke is 1px wide, high-contrast -> Foreground; the fill is large and
    // not thin -> Background.
    #[test]
    fn thin_high_contrast_stroke_is_foreground() {
        let (w, h) = (20usize, 20usize);
        let mut indices = vec![0u32; w * h];
        for y in 5..15 {
            indices[y * w + 10] = 1; // vertical stroke at x=10, y in [5,15)
        }
        let palette = [
            Rgba::new(240, 240, 240, 255), // light fill
            Rgba::new(20, 20, 20, 255),    // dark ink stroke
        ];
        let out = classify_regions(w, h, &indices, &palette, ColorDepth::Rgba8);
        // Stroke pixels foreground.
        assert_eq!(out[5 * w + 10], RegionClass::Foreground);
        assert_eq!(out[9 * w + 10], RegionClass::Foreground);
        // Fill pixels background (corner + a cell far from the stroke).
        assert_eq!(out[0], RegionClass::Background);
        assert_eq!(out[10 * w + 2], RegionClass::Background);
    }

    // A uniform fill is one big low-contrast component -> all Background.
    #[test]
    fn uniform_fill_is_background() {
        let (w, h) = (4usize, 4usize);
        let indices = vec![0u32; w * h];
        let palette = [Rgba::new(200, 200, 200, 255)];
        let out = classify_regions(w, h, &indices, &palette, ColorDepth::Rgba8);
        assert!(out.iter().all(|&c| c == RegionClass::Background));
    }

    // A colored small blob (high chroma + small area) votes foreground even
    // without a strong contrast edge.
    #[test]
    fn small_colored_blob_is_foreground() {
        let (w, h) = (10usize, 10usize);
        let mut indices = vec![0u32; w * h];
        // single colored pixel near the middle
        indices[5 * w + 5] = 1;
        let palette = [
            Rgba::new(245, 245, 245, 255), // near-white fill (low chroma)
            Rgba::new(220, 30, 30, 255),   // saturated red (high chroma, small)
        ];
        let out = classify_regions(w, h, &indices, &palette, ColorDepth::Rgba8);
        assert_eq!(out[5 * w + 5], RegionClass::Foreground);
    }

    #[test]
    fn empty_frame_returns_empty() {
        let out = classify_regions(0, 0, &[], &[], ColorDepth::Rgba8);
        assert!(out.is_empty());
    }
}
