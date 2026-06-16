//! Region-aware dark-theme build (preprocess-first), gated behind `encode`.
//!
//! `build_regional` classifies the raw pixels (a text-region core grown into its
//! anti-aliasing shell), builds the light index split by `(color, text?)`, then
//! seeds the dark theme: Background via the per-color transform, text via the
//! continuous lightness inversion `crate::derive::text_dark_for`.

use alloc::vec::Vec;

use crate::{
    ColorDepth, DifError, DifImage, Frame, IndexWidth, RegionClass, Result, Rgba, Strategy, Theme,
    dark_color_for, derive_dark_base_color, derive_dark_palette, indexed_from_rgba8, text_dark_for,
};
use crate::{aa_detect, abilities, derive, regions};

/// OKLab distance under which two split dark colors merge back together.
#[cfg(feature = "encode")]
const MERGE_EPS: f64 = 0.02;

/// How a pixel's color is transformed for the dark theme: a region class. `Bg`
/// keeps the per-color transform; `Fg` (text / thin strokes + the grown AA shell)
/// gets the lightness inversion.
#[cfg(feature = "encode")]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Classifier {
    Bg,
    Fg,
}

/// Outcome of [`build_regional`]: palette sizes before and after the region-aware
/// split + merge.
#[cfg(feature = "encode")]
#[derive(Clone, Copy, Debug)]
pub struct RegionalReport {
    /// Distinct light colors before splitting.
    pub base_indices: u64,
    /// Palette length after split + feature-merge + capacity-merge.
    pub split_indices: u64,
}

/// Merge provisional split entries to fit the palette. Each entry `i` has source
/// color id `cid[i]`, frequency `freq[i]`, and provisional `prov_light`/`prov_dark`
/// colors. First a **feature merge** collapses entries that share a color id and
/// have near-identical dark colors (within [`MERGE_EPS`]); then, if still over
/// `capacity`, a **capacity merge** collapses the least-used non-primary splits
/// back to their color's primary (highest-frequency) entry. Returns, per
/// provisional index, the final compact index, plus the surviving light/dark
/// palettes in final order. Deterministic (entries are assumed sorted by the
/// caller).
#[cfg(feature = "encode")]
fn merge_splits(
    cid: &[u32],
    freq: &[u32],
    prov_light: &[Rgba],
    prov_dark: &[Rgba],
    depth: ColorDepth,
    capacity: usize,
) -> (Vec<u32>, Vec<Rgba>, Vec<Rgba>) {
    use rustc_hash::FxHashMap;

    let n = cid.len();
    // Precompute each provisional dark's OKLab once: the feature merge below would
    // otherwise reconvert both sides of every candidate pair (`oklab_close`).
    let max = depth.max_value() as f64;
    let dark_lab: Vec<[f64; 3]> = prov_dark
        .iter()
        .map(|&c| {
            let o = derive::to_oklab(c, max);
            [o.l, o.a, o.b]
        })
        .collect();
    let close = |i: usize, j: usize| -> bool {
        if prov_dark[i] == prov_dark[j] {
            return true;
        }
        let (a, b) = (dark_lab[i], dark_lab[j]);
        let d = (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
        d.sqrt() <= MERGE_EPS
    };

    // Feature merge: only entries sharing a color id can merge, so compare within
    // a per-`cid` bucket instead of scanning all kept entries (was O(kept^2); a
    // 16-bit split palette reached tens of thousands of entries). Bucket order
    // mirrors `kept` insertion order, so the first-match `break` is unchanged.
    let mut remap: Vec<u32> = (0..n as u32).collect();
    let mut kept: Vec<u32> = Vec::new();
    let mut by_cid: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
    for i in 0..n as u32 {
        let bucket = by_cid.entry(cid[i as usize]).or_default();
        let mut merged = None;
        for &j in bucket.iter() {
            if close(i as usize, j as usize) {
                merged = Some(j);
                break;
            }
        }
        match merged {
            Some(j) => remap[i as usize] = j,
            None => {
                kept.push(i);
                bucket.push(i);
            }
        }
    }

    if kept.len() > capacity {
        let mut primary: FxHashMap<u32, u32> = FxHashMap::default();
        for &i in &kept {
            primary.entry(cid[i as usize]).or_insert(i);
        }
        let mut mergeable: Vec<u32> = kept
            .iter()
            .copied()
            .filter(|&i| primary[&cid[i as usize]] != i)
            .collect();
        mergeable.sort_unstable_by_key(|&i| freq[i as usize]);
        let mut need = kept.len() - capacity;
        for &i in &mergeable {
            if need == 0 {
                break;
            }
            remap[i as usize] = primary[&cid[i as usize]];
            need -= 1;
        }
        kept.retain(|&i| remap[i as usize] == i);
    }

    fn root(mut x: u32, remap: &[u32]) -> u32 {
        while remap[x as usize] != x {
            x = remap[x as usize];
        }
        x
    }
    let mut final_of: Vec<u32> = alloc::vec![u32::MAX; n];
    let mut fin_light: Vec<Rgba> = Vec::with_capacity(kept.len());
    let mut fin_dark: Vec<Rgba> = Vec::with_capacity(kept.len());
    for i in 0..n as u32 {
        if root(i, &remap) == i {
            final_of[i as usize] = fin_light.len() as u32;
            fin_light.push(prov_light[i as usize]);
            fin_dark.push(prov_dark[i as usize]);
        }
    }
    let prov_to_final: Vec<u32> = (0..n as u32)
        .map(|i| final_of[root(i, &remap) as usize])
        .collect();
    (prov_to_final, fin_light, fin_dark)
}

/// Radius (in growth passes) the text core expands into its AA fringe.
#[cfg(feature = "encode")]
const TEXT_GROW: usize = 2;

/// OKLab lightness below which a text-classified pixel is treated as dark INK and
/// gets the lightness inversion. At or above this it is a light region (a mislabeled
/// fill or a glyph's enclosed counter) and keeps the Background transform instead of
/// inverting to black.
#[cfg(feature = "encode")]
const INK_L: f64 = 0.5;

/// Grow a text `core` mask into the surrounding anti-aliasing fringe. A non-core
/// pixel joins the text region only if it is an **edge** pixel (Sobel energy above
/// [`aa_detect::TAU_EDGE`]) adjacent (8-connectivity) to an already-text pixel.
/// Repeating this [`TEXT_GROW`] times captures a 1-2px AA shell around each glyph
/// while never crossing into a flat fill (whose interior has no edge energy), so
/// the whole glyph inverts cleanly and the fill stays put. Deterministic.
#[cfg(feature = "encode")]
fn grow_text_mask(
    w: usize,
    h: usize,
    core: &[RegionClass],
    energy: &[f64],
) -> alloc::vec::Vec<bool> {
    let n = w * h;
    let mut mask = alloc::vec![false; n];
    for (p, slot) in mask.iter_mut().enumerate() {
        *slot = core[p] == RegionClass::Foreground;
    }
    for _ in 0..TEXT_GROW {
        // Within a pass every `add[p]` reads the frozen `mask` (and `energy`) and
        // writes only its own slot, so the pass bands over threads identically to a
        // serial sweep; the `mask |= add` merge is the sequential barrier between
        // passes. `fill_rows` falls back to serial for small frames.
        let mut add = alloc::vec![false; n];
        aa_detect::fill_rows(&mut add, w, h, |x, y, p| {
            if mask[p] || energy[p] <= aa_detect::TAU_EDGE {
                return false;
            }
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    if mask[ny as usize * w + nx as usize] {
                        return true;
                    }
                }
            }
            false
        });
        for (m, a) in mask.iter_mut().zip(add.iter()) {
            *m |= *a;
        }
    }
    mask
}

/// Build a two-theme (light + dark) image directly from raw RGBA, region-aware,
/// **preprocess-first**.
///
/// The taxonomy classification runs on the RAW pixels (background / foreground /
/// anti-aliasing) BEFORE the light palette is built, so the light index plane is
/// split by `(color, class)` from the start: the same source color used as a fill
/// and as a text fringe becomes two indices. Classifying the raw image -- not the
/// post-index, possibly quantized plane -- keeps anti-aliasing at full fidelity.
/// The light theme stays bit-exact (every index renders its pixel's original
/// color); only the dark colors differ. The dark theme is seeded per class: a flat
/// region keeps the per-color transform, and an AA fringe **snaps to its nearest
/// solid endpoint's dark color** (the "nearby color"), never a per-pixel transform
/// that would invert the near-neutral fringe to black or a false hue.
///
/// The "complex" case -- more unique colors than the index width can hold (photos),
/// or a non-spatial strategy -- quantizes and uses the plain per-color dark with no
/// region split.
#[cfg(feature = "encode")]
pub fn build_regional(
    width: u32,
    height: u32,
    rgba: &[u8],
    want: Option<IndexWidth>,
    strategy: Strategy,
) -> Result<(DifImage, Option<u64>, RegionalReport)> {
    use rustc_hash::FxHashMap;

    let (w, h) = (width as usize, height as usize);
    let px = w * h;
    if rgba.len() != px * 4 {
        return Err(DifError::Invalid("rgba length != 4*width*height"));
    }
    let depth = ColorDepth::Rgba8;

    // Unique raw colors (full fidelity), frequency-ordered for determinism.
    let mut freq_map: FxHashMap<u32, u32> = FxHashMap::default();
    for chunk in rgba.chunks_exact(4) {
        let key = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        *freq_map.entry(key).or_insert(0) += 1;
    }
    let n_colors = freq_map.len() as u64;

    let index_width = match want {
        Some(wd) => wd,
        None => {
            let natural = IndexWidth::for_count(n_colors);
            if natural.supported() {
                natural
            } else {
                IndexWidth::Bit16
            }
        }
    };
    if !index_width.supported() {
        return Err(DifError::BadIndexWidth((index_width.bytes() * 8) as u8));
    }
    let capacity = index_width.capacity() as usize;
    let dark_base = derive_dark_base_color([255, 255, 255], strategy);

    // Complex case: more colors than the index can hold, or a non-spatial
    // strategy. Quantize + per-color dark, no region split.
    if strategy != Strategy::Arithmetic || n_colors as usize > capacity {
        let (mut img, source_colors) = indexed_from_rgba8(width, height, rgba, Some(index_width))?;
        let light = img.palettes[0].clone();
        let base_indices = light.len() as u64;
        let dark = derive_dark_palette(&light, strategy, depth);
        img.palettes.push(dark);
        img.themes.push(Theme {
            abilities: abilities::DARK,
            base_color: dark_base,
        });
        img.validate()?;
        return Ok((
            img,
            source_colors,
            RegionalReport {
                base_indices,
                split_indices: base_indices,
            },
        ));
    }

    // Dense color map (raw color -> id), frequency-ordered, + raw palette + the
    // per-pixel id plane. This is the lossless, full-resolution view the classifier
    // runs on.
    let mut order: Vec<(u32, u32)> = freq_map.iter().map(|(&k, &c)| (k, c)).collect();
    order.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut id_of: FxHashMap<u32, u32> = FxHashMap::default();
    let mut raw_palette: Vec<Rgba> = Vec::with_capacity(order.len());
    for (i, (key, _)) in order.iter().enumerate() {
        let b = key.to_le_bytes();
        raw_palette.push(Rgba::new(
            b[0] as u16,
            b[1] as u16,
            b[2] as u16,
            b[3] as u16,
        ));
        id_of.insert(*key, i as u32);
    }
    // Per-pixel color id: an independent read-only `id_of` lookup, so band it over
    // threads for large frames (serial under fill_rows' threshold).
    let mut dense_idx = alloc::vec![0u32; px];
    aa_detect::fill_rows(&mut dense_idx, w, h, |_x, _y, p| {
        let o = p * 4;
        id_of[&u32::from_le_bytes([rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]])]
    });

    // PREPROCESS: classify the raw pixels into a TEXT mask. The region pass finds
    // the glyph/thin-stroke cores (Foreground); the core is then GROWN into the
    // surrounding anti-aliasing fringe -- but only across edge pixels (high Sobel
    // energy), never into flat fill -- so the whole glyph plus its AA shell is one
    // text region, while adjacent solid fills stay background. This replaces the
    // per-pixel AA filter, which could not reliably catch text.
    let core = regions::classify_regions(w, h, &dense_idx, &raw_palette, depth);
    let (lp, cp) =
        aa_detect::oklab_lc_planes_indexed(&dense_idx, &raw_palette, depth.max_value() as f64);
    let energy = aa_detect::edge_energy_planes(&lp, &cp, w, h);
    let is_text = grow_text_mask(w, h, &core, &energy);
    let cls: Vec<Classifier> = (0..px)
        .map(|p| {
            if is_text[p] {
                Classifier::Fg
            } else {
                Classifier::Bg
            }
        })
        .collect();

    // Dark seed. Background keeps the per-color transform. Text (Foreground + its
    // grown AA shell) inverts lightness continuously (`text_dark_for`) -- but ONLY
    // where the pixel is actually dark INK (`L < INK_L`). A light region that the
    // region pass mislabels Foreground (a glyph's enclosed counter, a small light
    // fill) is not ink: inverting it would send it to black (a "black hole"). Such
    // light text-keys fall back to the Background transform, which lands a mid-tone
    // matching the surrounding fill. For neutral colors the two transforms agree
    // (both are `1 - L`), so the glyph fringe stays seamless.
    let maxv = depth.max_value() as f64;
    let dark_of = |id: u32| {
        dark_color_for(
            raw_palette[id as usize],
            strategy,
            RegionClass::Background,
            depth,
        )
    };
    let is_ink = |id: u32| derive::to_oklab(raw_palette[id as usize], maxv).l < INK_L;
    let seed_dark = |id: u32, c: Classifier| -> Rgba {
        match c {
            Classifier::Fg if is_ink(id) => text_dark_for(raw_palette[id as usize], depth),
            _ => dark_of(id),
        }
    };

    // Build the light index from composite `(color id, class)` keys.
    let mut counts: FxHashMap<(u32, Classifier), u32> = FxHashMap::default();
    for p in 0..px {
        *counts.entry((dense_idx[p], cls[p])).or_insert(0) += 1;
    }
    let mut keys: Vec<((u32, Classifier), u32)> = counts.into_iter().collect();
    keys.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut key_index: FxHashMap<(u32, Classifier), u32> = FxHashMap::default();
    let mut prov_light: Vec<Rgba> = Vec::with_capacity(keys.len());
    let mut prov_dark: Vec<Rgba> = Vec::with_capacity(keys.len());
    let mut cid: Vec<u32> = Vec::with_capacity(keys.len());
    let mut freq: Vec<u32> = Vec::with_capacity(keys.len());
    for (k, c) in &keys {
        key_index.insert(*k, prov_light.len() as u32);
        prov_light.push(raw_palette[k.0 as usize]);
        prov_dark.push(seed_dark(k.0, k.1));
        cid.push(k.0);
        freq.push(*c);
    }

    let (prov_to_final, fin_light, fin_dark) =
        merge_splits(&cid, &freq, &prov_light, &prov_dark, depth, capacity);
    let split_indices = fin_light.len() as u64;

    // Re-sort the merged palette by descending TOTAL color frequency (ties by
    // ascending packed light color), restoring the `indexed_from_rgba8` invariant
    // that the hottest color gets the lowest index. merge_splits emits entries in
    // provisional `(color, class)`-pair-frequency order; split + merge can leave a
    // color ranked by its largest single-class count instead of its total, so the
    // final plane is not strictly frequency-ordered. Relabel here so the low-index
    // (low-high-byte) runs match the non-regional path exactly.
    let nf = fin_light.len();
    let mut final_freq = alloc::vec![0u64; nf];
    for (&fin, &f) in prov_to_final.iter().zip(freq.iter()) {
        final_freq[fin as usize] += f as u64;
    }
    let pack = |c: &Rgba| -> u64 {
        ((c.r as u64) << 48) | ((c.g as u64) << 32) | ((c.b as u64) << 16) | c.a as u64
    };
    let mut order: Vec<u32> = (0..nf as u32).collect();
    order.sort_unstable_by(|&a, &b| {
        final_freq[b as usize]
            .cmp(&final_freq[a as usize])
            .then_with(|| pack(&fin_light[a as usize]).cmp(&pack(&fin_light[b as usize])))
    });
    let mut old_to_new = alloc::vec![0u32; nf];
    for (new, &old) in order.iter().enumerate() {
        old_to_new[old as usize] = new as u32;
    }
    let fin_light: Vec<Rgba> = order.iter().map(|&o| fin_light[o as usize]).collect();
    let fin_dark: Vec<Rgba> = order.iter().map(|&o| fin_dark[o as usize]).collect();

    // Final per-pixel index: independent read-only lookups (key_index ->
    // prov_to_final -> old_to_new), banded over threads for large frames.
    let mut indices = alloc::vec![0u64; px];
    aa_detect::fill_rows(&mut indices, w, h, |_x, _y, p| {
        let prov = key_index[&(dense_idx[p], cls[p])];
        old_to_new[prov_to_final[prov as usize] as usize] as u64
    });

    let img = DifImage {
        width,
        height,
        color_depth: depth,
        index_width,
        themes: alloc::vec![
            Theme {
                abilities: abilities::LIGHT,
                base_color: [255, 255, 255],
            },
            Theme {
                abilities: abilities::DARK,
                base_color: dark_base,
            },
        ],
        palettes: alloc::vec![fin_light, fin_dark],
        frames: alloc::vec![Frame {
            delay_us: 0,
            indices,
        }],
        replay_count: 1,
    };
    img.validate()?;
    Ok((
        img,
        None,
        RegionalReport {
            base_indices: n_colors,
            split_indices,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "encode")]
    #[test]
    fn merge_splits_feature_and_capacity() {
        // Color 0 has four entries: a primary (freq 9), a near-identical-dark split
        // (feature-merges into the primary), and two distinct-dark splits. Color 1
        // has one. With capacity 3, exactly one of color 0's distinct splits must
        // capacity-merge back (and the capacity loop hits its early `break` once
        // the budget is spent while another mergeable entry remains).
        let cid = [0u32, 0, 0, 0, 1];
        let freq = [9u32, 3, 5, 4, 4];
        let g = Rgba::new(50, 50, 50, 255);
        let light = [g, g, g, g, Rgba::new(200, 10, 10, 255)];
        let dark = [
            Rgba::new(180, 180, 180, 255), // 0: primary
            Rgba::new(181, 180, 180, 255), // 1: ~identical -> feature merge into 0
            Rgba::new(90, 90, 90, 255),    // 2: distinct (freq 5, survives)
            Rgba::new(120, 120, 120, 255), // 3: distinct (freq 4, capacity-merged)
            Rgba::new(40, 10, 10, 255),    // 4: color 1
        ];
        let (to_final, fin_light, fin_dark) =
            merge_splits(&cid, &freq, &light, &dark, ColorDepth::Rgba8, 3);
        assert_eq!(fin_light.len(), 3);
        assert_eq!(fin_dark.len(), 3);
        assert_eq!(
            to_final[0], to_final[1],
            "near-identical dark feature-merged"
        );
        assert_eq!(
            to_final[0], to_final[3],
            "lowest-freq split capacity-merged"
        );
        assert_ne!(to_final[2], to_final[0], "higher-freq split survives");
        assert_ne!(to_final[4], to_final[0], "other color is its own index");
        assert!(fin_light.contains(&Rgba::new(200, 10, 10, 255)));
    }

    // A black stroke on a light fill: black text/fill split, light bit-exact, and
    // the stroke inverts bright (text) while the fill stays dark.
    #[cfg(feature = "encode")]
    #[test]
    fn build_regional_text_inverts_light_lossless() {
        let (w, h) = (32u32, 32u32);
        let fill = [210u8, 225, 250, 255];
        let ink = [0u8, 0, 0, 255];
        let mut rgba = Vec::new();
        // A large fill (Background) with a thin vertical stroke (Foreground text).
        for _y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(if x == 15 || x == 16 { &ink } else { &fill });
            }
        }
        let (img, sc, rep) = build_regional(w, h, &rgba, None, Strategy::Arithmetic).unwrap();
        assert_eq!(img.palettes.len(), 2);
        assert_eq!(img.themes.len(), 2);
        assert_eq!(sc, None);
        assert!(rep.split_indices >= rep.base_indices);
        // Light theme is bit-exact: every index renders its pixel's source color.
        let (light, dark) = (&img.palettes[0], &img.palettes[1]);
        for (p, &idx) in img.frames[0].indices.iter().enumerate() {
            let c = light[idx as usize];
            let o = p * 4;
            assert_eq!(
                [c.r as u8, c.g as u8, c.b as u8, c.a as u8],
                [rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]
            );
        }
        // The ink stroke's dark is bright (inverted text); the fill's dark is dark.
        let lum = |c: Rgba| 0.299 * c.r as f64 + 0.587 * c.g as f64 + 0.114 * c.b as f64;
        let ink_idx = img.frames[0].indices[15] as usize;
        let fill_idx = img.frames[0].indices[0] as usize;
        // Ink (black text) inverts bright; the light fill (Background) takes the
        // per-color compress, so it is not inverted as far -- a distinct, darker dark.
        assert!(
            lum(dark[ink_idx]) > 180.0,
            "ink dark should be bright (text)"
        );
        assert!(
            lum(dark[fill_idx]) < lum(dark[ink_idx]),
            "fill (Bg) and ink (text) must get different darks"
        );
    }

    // More unique colors than an 8-bit index can hold -> the "complex" path:
    // quantize + per-color dark, no region split.
    #[cfg(feature = "encode")]
    #[test]
    fn build_regional_complex_path_quantizes_no_split() {
        let (w, h) = (20u32, 20u32);
        let mut rgba = Vec::new();
        // 20x20 distinct (r, g) pairs = 400 unique colors > 8-bit capacity (256).
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 12) as u8, (y * 12) as u8, 0, 255]);
            }
        }
        let (img, sc, rep) =
            build_regional(w, h, &rgba, Some(IndexWidth::Bit8), Strategy::Arithmetic).unwrap();
        assert_eq!(img.palettes.len(), 2);
        assert!(sc.is_some(), "over-capacity input must quantize");
        assert_eq!(
            rep.split_indices, rep.base_indices,
            "no split in complex path"
        );
    }

    // Keep strategy has no spatial component: complex path, single transform.
    #[cfg(feature = "encode")]
    #[test]
    fn build_regional_keep_is_identity_dark() {
        let (w, h) = (4u32, 4u32);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        let (img, _, _) = build_regional(w, h, &rgba, None, Strategy::Keep).unwrap();
        assert_eq!(img.palettes[0], img.palettes[1], "keep dark == light");
    }

    // build_regional input validation + width edges.
    #[cfg(feature = "encode")]
    #[test]
    fn build_regional_rejects_bad_length_and_width() {
        // rgba length mismatch.
        assert!(build_regional(2, 2, &[0u8; 8], None, Strategy::Arithmetic).is_err());
        // an unsupported forced width.
        let ok_len = vec![0u8; 4 * 4];
        assert!(
            build_regional(2, 2, &ok_len, Some(IndexWidth::Bit32), Strategy::Arithmetic).is_err()
        );
    }

    // More than 16-bit-many unique colors with auto width: the natural width is an
    // unsupported 32-bit, so it falls back to Bit16 and takes the complex path.
    #[cfg(feature = "encode")]
    #[test]
    fn build_regional_auto_width_over_16bit_falls_back_and_quantizes() {
        let (w, h) = (260u32, 260u32); // 67600 px, all-distinct colors > 65536
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for i in 0..(w * h) {
            rgba.extend_from_slice(&[
                (i & 255) as u8,
                ((i >> 8) & 255) as u8,
                ((i >> 16) & 255) as u8,
                255,
            ]);
        }
        let (img, sc, rep) = build_regional(w, h, &rgba, None, Strategy::Arithmetic).unwrap();
        assert_eq!(img.index_width, IndexWidth::Bit16);
        assert!(sc.is_some(), "over-16-bit input must quantize");
        assert_eq!(rep.split_indices, rep.base_indices);
    }

    // The text core grows into adjacent edge pixels (the AA shell) but not into a
    // flat fill (no edge energy there).
    #[cfg(feature = "encode")]
    #[test]
    fn grow_text_mask_follows_edges_not_fill() {
        let (w, h) = (9usize, 3usize);
        let mut core = alloc::vec![RegionClass::Background; w * h];
        let mut energy = alloc::vec![0.0f64; w * h];
        for y in 0..h {
            core[y * w + 4] = RegionClass::Foreground; // a 1px stroke
            energy[y * w + 3] = 0.2; // AA shell on both sides (edge energy)
            energy[y * w + 5] = 0.2;
        }
        let m = grow_text_mask(w, h, &core, &energy);
        assert!(m[4], "core stays text");
        assert!(m[3] && m[5], "grew into the AA shell");
        assert!(!m[1] && !m[7], "flat fill (no edge) not grown");
    }

    // Two colors with equal pixel counts: the frequency-tie -> key tie-break in the
    // deterministic color/key sort must run (and the build stays reproducible).
    #[cfg(feature = "encode")]
    #[test]
    fn build_regional_equal_frequency_tie_break_is_deterministic() {
        let (w, h) = (8u32, 8u32);
        let a = [20u8, 20, 20, 255];
        let b = [200u8, 200, 200, 255];
        let mut rgba = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(if x < 4 { &a } else { &b }); // 32 px each, tied
            }
        }
        let build = || {
            let (img, _, _) = build_regional(w, h, &rgba, None, Strategy::Arithmetic).unwrap();
            img
        };
        let img = build();
        assert_eq!(img.palettes.len(), 2);
        assert_eq!(img, build(), "tie-broken build must be reproducible");
    }
}
