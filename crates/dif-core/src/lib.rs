//! Core codec for **DIF** — the Diagram Image Format.
//!
//! DIF is a lossless, theme-aware raster format. A single file carries one or
//! more *themes* (e.g. light / dark / high-contrast); the decoder renders the
//! theme matching the host's preference, falling back to the first theme.
//!
//! Two content modes share one header:
//! - [`Content::Indexed`]: a per-theme RGBA palette plus a UTF-8-style
//!   variable-length index stream (see [`varint`]).
//! - [`Content::Grayscale`]: raw samples plus a per-theme 1-D tone LUT, so a
//!   near-black gray can be remapped to stay visible on a dark background.
//!
//! Serialization comes in two flavours: [`to_difr`]/[`from_difr`] (raw, magic
//! `DIFR`) and [`to_dif`]/[`from_dif`] (compressed container, magic `DIF1`).
//!
//! # Build features
//!
//! The crate is `no_std` + `alloc` by default (store / deflate / lz4). It always
//! needs a heap allocator — the codec decode windows are runtime-sized — so a
//! `no_std` binary linking this crate must install a `#[global_allocator]`.
//! `std` adds the Brotli codec; `native` adds zstd, a libdeflate encoder, and
//! the lzav C shim.

// `no_std` for the real library build; tests need std for the libtest harness.
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod codec;
#[cfg(feature = "derive")]
pub mod derive;
pub mod error;
pub mod format;
pub mod varint;

pub use codec::{from_dif, to_dif, to_dif_workers, CodecId};
#[cfg(feature = "derive")]
pub use derive::{derive_dark_lut, derive_dark_palette, Strategy};
pub use error::{DifError, Result};
pub use format::{from_difr, to_difr};

/// Which host appearance a theme is intended for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ModeTag {
    Light = 0,
    Dark = 1,
    HighContrast = 2,
}

impl ModeTag {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(ModeTag::Light),
            1 => Ok(ModeTag::Dark),
            2 => Ok(ModeTag::HighContrast),
            _ => Err(DifError::Invalid("unknown mode tag")),
        }
    }
}

/// Bit depth per sample/channel, shared by palette RGBA and grayscale samples.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SampleDepth {
    Eight,
    Sixteen,
}

impl SampleDepth {
    /// Number of distinct sample values (`256` or `65536`).
    pub fn levels(self) -> usize {
        match self {
            SampleDepth::Eight => 256,
            SampleDepth::Sixteen => 65536,
        }
    }
    /// Bytes used to store one sample/channel on disk.
    pub fn bytes(self) -> usize {
        match self {
            SampleDepth::Eight => 1,
            SampleDepth::Sixteen => 2,
        }
    }
    /// Largest representable sample value.
    pub fn max_value(self) -> u16 {
        match self {
            SampleDepth::Eight => 255,
            SampleDepth::Sixteen => 65535,
        }
    }
}

/// A named theme with the host appearance it targets.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Theme {
    pub tag: ModeTag,
    pub name: String,
}

/// An RGBA color. Channels are stored as `u16` to cover both 8- and 16-bit
/// depth uniformly; for 8-bit depth values stay in `0..=255`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgba {
    pub r: u16,
    pub g: u16,
    pub b: u16,
    pub a: u16,
}

impl Rgba {
    pub const fn new(r: u16, g: u16, b: u16, a: u16) -> Self {
        Rgba { r, g, b, a }
    }
}

/// Pixel content. `Indexed` and `Grayscale` are the two modes; both are
/// per-theme.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Content {
    Indexed {
        /// `palettes[theme]` is the full palette for that theme; every theme's
        /// palette has the same length (`color_count`).
        palettes: Vec<Vec<Rgba>>,
        /// `frames[f]` holds `width*height` palette indices, row-major.
        frames: Vec<Vec<u32>>,
    },
    Grayscale {
        /// `luts[theme]` maps a stored sample value to the themed value. Length
        /// equals `depth.levels()`. The first theme's LUT is usually identity.
        luts: Vec<Vec<u16>>,
        /// `frames[f]` holds `width*height` raw samples, row-major.
        frames: Vec<Vec<u16>>,
    },
}

/// A complete DIF image: header + themes + content + per-frame delays.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DifImage {
    pub width: u32,
    pub height: u32,
    pub depth: SampleDepth,
    pub themes: Vec<Theme>,
    pub content: Content,
    /// Per-frame display delay in milliseconds; `0` for a static image.
    pub frame_delays: Vec<u16>,
}

impl DifImage {
    pub fn frame_count(&self) -> usize {
        match &self.content {
            Content::Indexed { frames, .. } => frames.len(),
            Content::Grayscale { frames, .. } => frames.len(),
        }
    }

    fn pixels_per_frame(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Pick the theme index whose tag matches `prefer`, else theme 0.
    pub fn theme_for(&self, prefer: ModeTag) -> usize {
        self.themes
            .iter()
            .position(|t| t.tag == prefer)
            .unwrap_or(0)
    }

    /// Validate cross-field invariants. Called by the encoders.
    pub fn validate(&self) -> Result<()> {
        let n = self.themes.len();
        if n == 0 || n > 128 {
            return Err(DifError::BadThemeCount(n));
        }
        let px = self.pixels_per_frame();
        match &self.content {
            Content::Indexed { palettes, frames } => {
                if palettes.len() != n {
                    return Err(DifError::Invalid("palette count != theme count"));
                }
                let cc = palettes[0].len();
                if palettes.iter().any(|p| p.len() != cc) {
                    return Err(DifError::Invalid("themes have differing palette sizes"));
                }
                if frames.iter().any(|f| f.len() != px) {
                    return Err(DifError::Invalid("frame size != width*height"));
                }
                if frames.iter().any(|f| f.iter().any(|&i| i as usize >= cc)) {
                    return Err(DifError::Invalid("palette index out of range"));
                }
            }
            Content::Grayscale { luts, frames } => {
                if luts.len() != n {
                    return Err(DifError::Invalid("lut count != theme count"));
                }
                let levels = self.depth.levels();
                if luts.iter().any(|l| l.len() != levels) {
                    return Err(DifError::Invalid("lut length != depth levels"));
                }
                if frames.iter().any(|f| f.len() != px) {
                    return Err(DifError::Invalid("frame size != width*height"));
                }
            }
        }
        Ok(())
    }

    /// Render `frame` under the theme matching `prefer` into packed RGBA8
    /// (`4 * width * height` bytes), suitable for a browser canvas. 16-bit
    /// content is scaled down to 8-bit for display.
    pub fn render_rgba8(&self, prefer: ModeTag, frame: usize) -> Result<Vec<u8>> {
        let t = self.theme_for(prefer);
        let px = self.pixels_per_frame();
        let scale = |v: u16| -> u8 {
            match self.depth {
                SampleDepth::Eight => v as u8,
                SampleDepth::Sixteen => (v >> 8) as u8,
            }
        };
        // Bake depth-scaling into a small RGBA8 lookup table once (palette/lut
        // size, cache-resident), so the per-pixel loop is a branch-free copy.
        let mut out = alloc::vec![0u8; px * 4];
        match &self.content {
            Content::Indexed { palettes, frames } => {
                let pal = &palettes[t];
                let f = frames.get(frame).ok_or(DifError::Invalid("frame index"))?;
                let lut: Vec<[u8; 4]> = pal
                    .iter()
                    .map(|c| [scale(c.r), scale(c.g), scale(c.b), scale(c.a)])
                    .collect();
                for (dst, &idx) in out.chunks_exact_mut(4).zip(f) {
                    dst.copy_from_slice(&lut[idx as usize]);
                }
            }
            Content::Grayscale { luts, frames } => {
                let f = frames.get(frame).ok_or(DifError::Invalid("frame index"))?;
                let lut: Vec<[u8; 4]> = luts[t]
                    .iter()
                    .map(|&v| {
                        let g = scale(v);
                        [g, g, g, 0xFF]
                    })
                    .collect();
                for (dst, &s) in out.chunks_exact_mut(4).zip(f) {
                    dst.copy_from_slice(&lut[s as usize]);
                }
            }
        }
        Ok(out)
    }
}

/// Build a single-theme (light) indexed image straight from a packed RGBA8
/// buffer (`4 * width * height` bytes, row-major). Dedups colors into a palette
/// and emits the index frame natively, so callers (e.g. the Python binding) keep
/// the per-pixel work in Rust instead of marshalling a million-element index list
/// across the FFI boundary. Add further themes (e.g. a derived dark palette)
/// afterwards.
///
/// The palette is ordered by **descending color frequency** (ties broken by
/// ascending packed RGBA key, so output bytes are reproducible): the hottest
/// colors get the lowest indices and therefore the shortest varints in the index
/// stream (`0..=127` -> 1 byte), which shrinks the body on images with more than
/// 128 distinct colors. This costs a second pass over the pixels. Decode is
/// unaffected — palette order carries no semantics.
pub fn indexed_from_rgba8(
    width: u32,
    height: u32,
    depth: SampleDepth,
    rgba: &[u8],
) -> Result<DifImage> {
    // FxHash `HashMap` under `std` (the integer key makes SipHash pure overhead);
    // fall back to alloc's `BTreeMap` on the default `no_std` build. Iteration
    // order differs, but the explicit sort below makes the final palette order
    // identical (and deterministic) either way.
    #[cfg(feature = "std")]
    use rustc_hash::FxHashMap as ColorMap;
    #[cfg(not(feature = "std"))]
    use alloc::collections::BTreeMap as ColorMap;

    let px = width as usize * height as usize;
    if rgba.len() != px * 4 {
        return Err(DifError::Invalid("rgba length != 4*width*height"));
    }

    // Pass 1: tally how often each packed color occurs.
    let mut map: ColorMap<u32, u32> = ColorMap::default();
    for chunk in rgba.chunks_exact(4) {
        let key = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        *map.entry(key).or_insert(0) += 1;
    }

    // Order colors by frequency (desc), tie-break by key (asc) for determinism.
    let mut order: Vec<(u32, u32)> = map.iter().map(|(&k, &c)| (k, c)).collect();
    order.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // Materialize the palette and repurpose `map` as color -> final index.
    let mut palette: Vec<Rgba> = Vec::with_capacity(order.len());
    for (idx, (key, _)) in order.iter().enumerate() {
        let b = key.to_le_bytes();
        palette.push(Rgba::new(b[0] as u16, b[1] as u16, b[2] as u16, b[3] as u16));
        map.insert(*key, idx as u32);
    }

    // Pass 2: emit the index frame against the frequency-ordered palette.
    let mut frame: Vec<u32> = Vec::with_capacity(px);
    for chunk in rgba.chunks_exact(4) {
        let key = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        frame.push(map[&key]);
    }

    let palettes: Vec<Vec<Rgba>> = alloc::vec![palette];
    let frames: Vec<Vec<u32>> = alloc::vec![frame];
    let themes: Vec<Theme> = alloc::vec![Theme {
        tag: ModeTag::Light,
        name: String::from("light"),
    }];
    let img = DifImage {
        width,
        height,
        depth,
        themes,
        content: Content::Indexed { palettes, frames },
        frame_delays: Vec::new(),
    };
    img.validate()?;
    Ok(img)
}

/// Build a single-theme (light) grayscale image straight from a packed sample
/// buffer (row-major). 8-bit samples are one byte each; 16-bit samples are
/// **little-endian** `u16` pairs (`2 * width * height` bytes). The light theme
/// gets an identity LUT; add a derived dark LUT afterwards. Mirrors
/// [`indexed_from_rgba8`] so the Python binding hands over the raw bitmap instead
/// of marshalling a per-pixel sample list across the FFI boundary. `alloc`-only.
pub fn grayscale_from_samples(
    width: u32,
    height: u32,
    depth: SampleDepth,
    samples: &[u8],
) -> Result<DifImage> {
    let px = width as usize * height as usize;
    if samples.len() != px * depth.bytes() {
        return Err(DifError::Invalid("samples length != bytes*width*height"));
    }
    let frame: Vec<u16> = match depth {
        SampleDepth::Eight => samples.iter().map(|&b| b as u16).collect(),
        SampleDepth::Sixteen => samples
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect(),
    };
    let identity: Vec<u16> = (0..depth.levels()).map(|v| v as u16).collect();
    let luts: Vec<Vec<u16>> = alloc::vec![identity];
    let frames: Vec<Vec<u16>> = alloc::vec![frame];
    let themes: Vec<Theme> = alloc::vec![Theme {
        tag: ModeTag::Light,
        name: String::from("light"),
    }];
    let img = DifImage {
        width,
        height,
        depth,
        themes,
        content: Content::Grayscale { luts, frames },
        frame_delays: Vec::new(),
    };
    img.validate()?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Content, Rgba, SampleDepth};

    fn indexed_parts(img: &DifImage) -> (&Vec<Rgba>, &Vec<u32>) {
        match &img.content {
            Content::Indexed { palettes, frames } => (&palettes[0], &frames[0]),
            _ => panic!("expected indexed content"),
        }
    }

    // Color A is used 3x, B 1x, but B is the FIRST pixel. First-appearance order
    // (the old behavior) would put B at index 0; frequency order must put the
    // hotter color A at index 0.
    #[test]
    fn palette_ordered_by_frequency_not_first_seen() {
        let a = [10u8, 20, 30, 255];
        let b = [200u8, 100, 50, 255];
        // Pixel order: B, A, A, A
        let rgba: Vec<u8> = [b, a, a, a].concat();
        let img = indexed_from_rgba8(2, 2, SampleDepth::Eight, &rgba).unwrap();
        let (palette, frame) = indexed_parts(&img);

        assert_eq!(palette[0], Rgba::new(10, 20, 30, 255), "hottest color first");
        assert_eq!(palette[1], Rgba::new(200, 100, 50, 255));
        assert_eq!(frame, &alloc::vec![1u32, 0, 0, 0], "indices reference freq palette");
    }

    // Equal counts must break ties by ascending packed key, so output is
    // deterministic regardless of pixel order or the backing map type.
    #[test]
    fn equal_frequency_breaks_ties_by_key() {
        let lo = [0u8, 0, 0, 255]; // packed key 0xFF00_0000
        let hi = [1u8, 0, 0, 255]; // packed key 0xFF00_0001 (larger)
                                   // Higher-key color appears first; counts are equal (1 each).
        let rgba: Vec<u8> = [hi, lo].concat();
        let img = indexed_from_rgba8(2, 1, SampleDepth::Eight, &rgba).unwrap();
        let (palette, frame) = indexed_parts(&img);

        assert_eq!(palette[0], Rgba::new(0, 0, 0, 255), "lower key wins the tie");
        assert_eq!(palette[1], Rgba::new(1, 0, 0, 255));
        assert_eq!(frame, &alloc::vec![1u32, 0]);
    }

    // palette[frame[i]] must reproduce the original pixel for every pixel.
    #[test]
    fn frame_reconstructs_original_pixels() {
        let px: [[u8; 4]; 5] = [
            [9, 9, 9, 255],
            [9, 9, 9, 255],
            [200, 0, 0, 128],
            [9, 9, 9, 255],
            [200, 0, 0, 128],
        ];
        let rgba: Vec<u8> = px.iter().flatten().copied().collect();
        let img = indexed_from_rgba8(5, 1, SampleDepth::Eight, &rgba).unwrap();
        let (palette, frame) = indexed_parts(&img);

        for (i, want) in px.iter().enumerate() {
            let got = palette[frame[i] as usize];
            assert_eq!(
                got,
                Rgba::new(want[0] as u16, want[1] as u16, want[2] as u16, want[3] as u16),
                "pixel {i} round-trips",
            );
        }
    }
}
