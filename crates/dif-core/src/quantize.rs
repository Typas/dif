//! Encode-side palette reduction (OKLab median-cut), gated behind the `derive`
//! feature (it shares the `palette` crate's sRGB/OKLab conversion with
//! [`crate::derive`]).
//!
//! When an image has more unique colors than the chosen [`crate::IndexWidth`] can
//! index, the builder calls [`quantize_oklab`] to merge colors down to fit instead
//! of failing — the lossy fallback that lets a photographic source still encode as
//! an indexed `.dif` (parity with GIF's 256-color palette quantize). Colors are
//! clustered by **median-cut in OKLab space** so merges follow perceived color
//! distance rather than raw sRGB. Alpha is carried as a 4th box axis so translucent
//! colors are not collapsed into opaque ones.
//!
//! Determinism: every tie (which box to split, where to split, box ordering) is
//! broken by the packed color key, so the output palette + remap are byte-for-byte
//! reproducible — same contract as the rest of the encoder.

use alloc::vec::Vec;
use core::cmp::Ordering;
use std::collections::BinaryHeap;

use palette::{IntoColor, Oklab, Srgb};
use rustc_hash::FxHashMap;

/// Weight applied to the (0..1) alpha axis relative to the OKLab color axes in the
/// median-cut distance. `L` spans ~0..1 and `a`/`b` ~ -0.4..0.4, so a weight of
/// 2.0 makes a full opaque/transparent gap the dominant axis — transparent and
/// opaque colors never share a cluster.
const ALPHA_WEIGHT: f64 = 2.0;

/// One source color: its packed key, frequency, and 4D coordinate
/// `(L, a, b, alpha*ALPHA_WEIGHT)` used for clustering.
struct Item {
    key: u32,
    freq: u32,
    coord: [f64; 4],
}

/// Unpack a little-endian RGBA8 key (matching the builder's
/// `u32::from_le_bytes([r, g, b, a])`) into its OKLab+alpha coordinate.
fn coord_of(key: u32) -> [f64; 4] {
    let [r, g, b, a] = key.to_le_bytes();
    let lab: Oklab<f64> =
        Srgb::new(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0).into_color();
    [lab.l, lab.a, lab.b, a as f64 / 255.0 * ALPHA_WEIGHT]
}

/// The frequency-weighted mean coordinate of a box, mapped back to a packed
/// RGBA8 key (OKLab axes -> sRGB8, alpha un-weighted and rounded).
fn representative(items: &[Item], idxs: &[usize]) -> u32 {
    let mut sum = [0.0f64; 4];
    let mut w = 0.0f64;
    for &i in idxs {
        let f = items[i].freq as f64;
        for (s, &cv) in sum.iter_mut().zip(&items[i].coord) {
            *s += cv * f;
        }
        w += f;
    }
    for c in &mut sum {
        *c /= w;
    }
    let srgb: Srgb<f64> = Oklab::new(sum[0], sum[1], sum[2]).into_color();
    let enc = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let a = (sum[3] / ALPHA_WEIGHT).clamp(0.0, 1.0) * 255.0;
    u32::from_le_bytes([
        enc(srgb.red),
        enc(srgb.green),
        enc(srgb.blue),
        a.round() as u8,
    ])
}

/// Per-axis `(min, max)` extent of a box.
fn extents(items: &[Item], idxs: &[usize]) -> [(f64, f64); 4] {
    let mut ext = [(f64::INFINITY, f64::NEG_INFINITY); 4];
    for &i in idxs {
        for (e, &v) in ext.iter_mut().zip(&items[i].coord) {
            e.0 = e.0.min(v);
            e.1 = e.1.max(v);
        }
    }
    ext
}

/// A box on the split heap: ordered by greatest single-axis range (so the most
/// spread-out cluster is split next), tie-broken by the smallest member key for
/// determinism.
struct HeapBox {
    range_bits: u64, // max axis range, as f64::to_bits (monotonic for >= 0)
    min_key: u32,    // deterministic tie-break
    axis: usize,     // axis carrying that range
    idxs: Vec<usize>,
}

impl HeapBox {
    fn new(items: &[Item], idxs: Vec<usize>) -> Self {
        let ext = extents(items, &idxs);
        let (axis, range) = (0..4)
            .map(|c| (c, ext[c].1 - ext[c].0))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let min_key = idxs.iter().map(|&i| items[i].key).min().unwrap();
        HeapBox {
            range_bits: range.to_bits(),
            min_key,
            axis,
            idxs,
        }
    }
    /// Splittable only if it holds >= 2 colors spread over a non-zero range.
    fn splittable(&self) -> bool {
        self.idxs.len() >= 2 && self.range_bits > 0.0f64.to_bits()
    }
}

impl PartialEq for HeapBox {
    fn eq(&self, o: &Self) -> bool {
        self.range_bits == o.range_bits && self.min_key == o.min_key
    }
}
impl Eq for HeapBox {}
impl Ord for HeapBox {
    fn cmp(&self, o: &Self) -> Ordering {
        // Max-heap on range, then a stable tie-break so pops are deterministic.
        self.range_bits
            .cmp(&o.range_bits)
            .then(o.min_key.cmp(&self.min_key))
    }
}
impl PartialOrd for HeapBox {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Split a box at the frequency-weighted median of its widest axis. Members are
/// ordered along that axis (tie-broken by key) and cut where cumulative frequency
/// first reaches half the box total.
fn split(items: &[Item], mut b: HeapBox) -> (HeapBox, HeapBox) {
    let axis = b.axis;
    b.idxs.sort_unstable_by(|&x, &y| {
        items[x].coord[axis]
            .total_cmp(&items[y].coord[axis])
            .then(items[x].key.cmp(&items[y].key))
    });
    let total: u64 = b.idxs.iter().map(|&i| items[i].freq as u64).sum();
    let mut acc = 0u64;
    let mut cut = 1; // at least one element on each side
    for (pos, &i) in b.idxs.iter().enumerate() {
        acc += items[i].freq as u64;
        if acc * 2 >= total {
            cut = (pos + 1).min(b.idxs.len() - 1).max(1);
            break;
        }
    }
    let right = b.idxs.split_off(cut);
    (HeapBox::new(items, b.idxs), HeapBox::new(items, right))
}

/// Reduce `counts` (packed color key -> frequency) to at most `capacity` colors by
/// OKLab median-cut, **in place**: on return `counts` holds only the representative
/// keys mapped to their summed frequencies (ready for the builder's freq-desc
/// reorder). The returned map sends every original key to its representative key,
/// for the second (index-emit) pass. `capacity` must be >= 1.
pub fn quantize_oklab(counts: &mut FxHashMap<u32, u32>, capacity: usize) -> FxHashMap<u32, u32> {
    let items: Vec<Item> = counts
        .iter()
        .map(|(&key, &freq)| Item {
            key,
            freq,
            coord: coord_of(key),
        })
        .collect();

    // Median-cut: pop the widest box, split it, until we have `capacity` boxes (or
    // nothing left is splittable).
    let mut heap = BinaryHeap::new();
    heap.push(HeapBox::new(&items, (0..items.len()).collect()));
    let mut finals: Vec<HeapBox> = Vec::new();
    while finals.len() + heap.len() < capacity {
        let Some(top) = heap.pop() else { break };
        if !top.splittable() {
            finals.push(top);
            continue;
        }
        let (l, r) = split(&items, top);
        heap.push(l);
        heap.push(r);
    }
    finals.extend(heap);

    // Build the representative palette + per-original-key remap, and rewrite
    // `counts` to representative -> summed frequency (summing on key collisions).
    let mut subst: FxHashMap<u32, u32> =
        FxHashMap::with_capacity_and_hasher(items.len(), Default::default());
    counts.clear();
    for b in &finals {
        let rep = representative(&items, &b.idxs);
        let mut box_freq = 0u32;
        for &i in &b.idxs {
            subst.insert(items[i].key, rep);
            box_freq = box_freq.saturating_add(items[i].freq);
        }
        let slot = counts.entry(rep).or_insert(0);
        *slot = slot.saturating_add(box_freq);
    }
    subst
}
