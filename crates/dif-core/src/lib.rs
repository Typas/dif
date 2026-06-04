//! Core codec for **DIF v3** — the Diagram Image Format.
//!
//! DIF is a lossless, theme-aware, palette-indexed raster format. A single file
//! carries one or more *themes*; each theme is a full palette plus an
//! [`abilities`] bitmask (which host appearances it can display under) and a
//! `base_color`. The decoder picks the theme matching the host appearance and
//! background (see [`DifImage::pick_theme`]).
//!
//! v3 vs v2: grayscale mode and the UTF-8-style varint index are gone. Indices
//! are a constant-width plane (8- or 16-bit), the mapped color is RGBA8 or
//! RGBA16, and the body uses a two-stage codec (per-palette + per-frame sections
//! wrapped by an outer pass) so a decoder can inflate one palette / one frame on
//! demand. See [`codec`] for the 64-byte container and [`format`] for the body.
//!
//! # Build features
//!
//! `no_std` + `alloc` by default (store / deflate / lz4). `std` adds Brotli;
//! `native` adds zstd + a libdeflate encoder + the lzav C shim; `derive` adds the
//! encode-side dark-theme derivation.

// `no_std` for the real library build; tests need std for the libtest harness.
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

extern crate alloc;

use alloc::vec::Vec;

pub mod codec;
#[cfg(feature = "derive")]
pub mod derive;
pub mod error;
pub mod format;
#[cfg(feature = "derive")]
pub mod quantize;

pub use codec::{
    from_dif, from_dif_workers, from_difr, to_dif, to_dif_workers, to_difr, Codec, CodecId,
};
#[cfg(feature = "derive")]
pub use derive::{derive_dark_base_color, derive_dark_palette, Strategy};
pub use error::{DifError, Result};

/// Theme capability bits (the low 3 bits of a theme's `abilities` byte). The top
/// 5 bits are reserved and must be zero.
pub mod abilities {
    /// The theme can be displayed under a light host appearance.
    pub const LIGHT: u8 = 1 << 0;
    /// The theme can be displayed under a dark host appearance.
    pub const DARK: u8 = 1 << 1;
    /// The theme is suitable for a high-contrast host appearance.
    pub const HIGH_CONTRAST: u8 = 1 << 2;
    /// Mask of all currently-defined capability bits.
    pub const ALL: u8 = LIGHT | DARK | HIGH_CONTRAST;
}

/// A host appearance the caller is rendering for. Maps to one capability bit when
/// picking a theme.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeTag {
    Light,
    Dark,
    HighContrast,
}

impl ThemeTag {
    /// The `abilities` bit a theme must set to be capable for this appearance.
    pub fn ability_bit(self) -> u8 {
        match self {
            ThemeTag::Light => abilities::LIGHT,
            ThemeTag::Dark => abilities::DARK,
            ThemeTag::HighContrast => abilities::HIGH_CONTRAST,
        }
    }
}

/// Width of one palette index in the constant-width index plane.
///
/// All four widths the flags byte can encode are named, but only 8- and 16-bit are
/// supported by this build (see [`IndexWidth::supported`]); a [`DifImage`] carrying
/// [`Bit32`](IndexWidth::Bit32)/[`Bit64`](IndexWidth::Bit64) is rejected by
/// [`DifImage::validate`]. The encoder never produces them — it quantizes down to
/// the widest supported width instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IndexWidth {
    Bit8,
    Bit16,
    Bit32,
    Bit64,
}

impl IndexWidth {
    /// Bytes used to store one index on disk.
    pub fn bytes(self) -> usize {
        match self {
            IndexWidth::Bit8 => 1,
            IndexWidth::Bit16 => 2,
            IndexWidth::Bit32 => 4,
            IndexWidth::Bit64 => 8,
        }
    }
    /// Number of distinct indices representable (`256`, `65536`, `1<<32`, or — for
    /// 64-bit — `u64::MAX`, capped to avoid overflowing the `u64` count space).
    pub fn capacity(self) -> u64 {
        match self {
            IndexWidth::Bit8 => 256,
            IndexWidth::Bit16 => 65536,
            IndexWidth::Bit32 => 1 << 32,
            IndexWidth::Bit64 => u64::MAX,
        }
    }
    /// Whether this build can encode/decode the width. Only 8- and 16-bit are
    /// wired up; 32-/64-bit are named for the flags layout but not implemented.
    pub fn supported(self) -> bool {
        matches!(self, IndexWidth::Bit8 | IndexWidth::Bit16)
    }
    /// The flags byte's two low bits for this width.
    pub fn to_bits(self) -> u8 {
        match self {
            IndexWidth::Bit8 => 0b00,
            IndexWidth::Bit16 => 0b01,
            IndexWidth::Bit32 => 0b10,
            IndexWidth::Bit64 => 0b11,
        }
    }
    /// Parse from the flags byte's two low bits. All four are recognized; the
    /// unsupported ones (`Bit32`/`Bit64`) are rejected later by [`DifImage::validate`].
    pub fn from_bits(bits: u8) -> Result<Self> {
        Ok(match bits & 0b11 {
            0b00 => IndexWidth::Bit8,
            0b01 => IndexWidth::Bit16,
            0b10 => IndexWidth::Bit32,
            _ => IndexWidth::Bit64,
        })
    }
    /// The smallest width that *would* hold `count` colors — a suggestion, not a
    /// guarantee it is supported. The encoder uses this to learn the natural width
    /// and, when it exceeds the widest supported width, quantizes down to fit.
    pub fn for_count(count: u64) -> Self {
        if count <= 256 {
            IndexWidth::Bit8
        } else if count <= 65536 {
            IndexWidth::Bit16
        } else if count <= 1 << 32 {
            IndexWidth::Bit32
        } else {
            IndexWidth::Bit64
        }
    }
}

/// Mapped-color channel depth: RGBA with 8- or 16-bit channels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorDepth {
    Rgba8,
    Rgba16,
}

impl ColorDepth {
    /// Bytes used to store one color channel on disk.
    pub fn channel_bytes(self) -> usize {
        match self {
            ColorDepth::Rgba8 => 1,
            ColorDepth::Rgba16 => 2,
        }
    }
    /// Bytes used to store one RGBA color on disk.
    pub fn color_bytes(self) -> usize {
        4 * self.channel_bytes()
    }
    /// Largest representable channel value.
    pub fn max_value(self) -> u16 {
        match self {
            ColorDepth::Rgba8 => 255,
            ColorDepth::Rgba16 => 65535,
        }
    }
    /// The flags byte's color nibble (bits 2..=5) for this depth.
    pub fn to_bits(self) -> u8 {
        match self {
            ColorDepth::Rgba8 => 0x0,
            ColorDepth::Rgba16 => 0x1,
        }
    }
    /// Parse from the flags byte's color nibble (bits 2..=5).
    pub fn from_bits(bits: u8) -> Result<Self> {
        match bits & 0xF {
            0x0 => Ok(ColorDepth::Rgba8),
            0x1 => Ok(ColorDepth::Rgba16),
            other => Err(DifError::BadColorDepth(other)),
        }
    }
}

/// A named theme's capability + base color. The palette itself lives in
/// [`DifImage::palettes`] at the matching index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    /// Capability bits — see [`abilities`].
    pub abilities: u8,
    /// The scheme's base (background) color, RGB8, used to tie-break the pick.
    pub base_color: [u8; 3],
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

/// One animation frame: a display delay plus the row-major index plane.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    /// Display delay in microseconds; `0` for a static frame.
    pub delay_us: u32,
    /// `width * height` palette indices, row-major. Held as `u64` in memory;
    /// serialized at the image's [`IndexWidth`].
    pub indices: Vec<u64>,
}

/// A complete DIF image: dimensions + themes + per-theme palettes + frames.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DifImage {
    pub width: u32,
    pub height: u32,
    pub color_depth: ColorDepth,
    pub index_width: IndexWidth,
    pub themes: Vec<Theme>,
    /// `palettes[theme]` is that theme's full palette; every theme's palette has
    /// the same length (`index_count`).
    pub palettes: Vec<Vec<Rgba>>,
    pub frames: Vec<Frame>,
    /// How many times to replay the animation: `0` = infinite, `1` = static.
    pub replay_count: u16,
}

impl DifImage {
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Number of colors in each theme's palette.
    pub fn index_count(&self) -> usize {
        self.palettes.first().map_or(0, |p| p.len())
    }

    fn pixels_per_frame(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Pick the theme best matching `prefer` and the host `base_color`.
    ///
    /// Among themes whose abilities cover `prefer`, the one with the nearest
    /// `base_color` (squared RGB distance) wins; ties resolve to the lowest index.
    /// If no theme is capable, falls back to theme 0.
    pub fn pick_theme(&self, prefer: ThemeTag, base_color: [u8; 3]) -> usize {
        let bit = prefer.ability_bit();
        let dist = |c: [u8; 3]| -> u32 {
            let d = |x: u8, y: u8| {
                let v = x as i32 - y as i32;
                (v * v) as u32
            };
            d(c[0], base_color[0]) + d(c[1], base_color[1]) + d(c[2], base_color[2])
        };
        let mut best: Option<(usize, u32)> = None;
        for (i, t) in self.themes.iter().enumerate() {
            if t.abilities & bit != 0 {
                let d = dist(t.base_color);
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((i, d));
                }
            }
        }
        best.map_or(0, |(i, _)| i)
    }

    /// Validate cross-field invariants. Called by the encoders.
    pub fn validate(&self) -> Result<()> {
        let n = self.themes.len();
        if n == 0 || n > 256 {
            return Err(DifError::BadThemeCount(n));
        }
        for t in &self.themes {
            if t.abilities & !abilities::ALL != 0 {
                return Err(DifError::BadAbilities(t.abilities));
            }
        }
        if self.palettes.len() != n {
            return Err(DifError::Invalid("palette count != theme count"));
        }
        let cc = self.index_count();
        if cc == 0 {
            return Err(DifError::Invalid("palette is empty"));
        }
        if self.palettes.iter().any(|p| p.len() != cc) {
            return Err(DifError::Invalid("themes have differing palette sizes"));
        }
        if !self.index_width.supported() {
            return Err(DifError::BadIndexWidth(
                (self.index_width.bytes() * 8) as u8,
            ));
        }
        if cc as u64 > self.index_width.capacity() {
            return Err(DifError::Invalid(
                "index_count exceeds index width capacity",
            ));
        }
        let maxv = self.color_depth.max_value();
        if self
            .palettes
            .iter()
            .flatten()
            .any(|c| c.r > maxv || c.g > maxv || c.b > maxv || c.a > maxv)
        {
            return Err(DifError::Invalid("palette color exceeds color depth"));
        }
        if self.frames.is_empty() {
            return Err(DifError::Invalid("image has no frames"));
        }
        if self.frames.len() > u16::MAX as usize {
            return Err(DifError::Invalid("frame count exceeds u16"));
        }
        let px = self.pixels_per_frame();
        if self.frames.iter().any(|f| f.indices.len() != px) {
            return Err(DifError::Invalid("frame size != width*height"));
        }
        if self
            .frames
            .iter()
            .any(|f| f.indices.iter().any(|&i| i >= cc as u64))
        {
            return Err(DifError::Invalid("palette index out of range"));
        }
        Ok(())
    }

    /// Render `frame` under the theme matching `prefer` + host `base_color` into
    /// packed RGBA8 (`4 * width * height` bytes). 16-bit color is scaled to 8-bit.
    pub fn render_rgba8(
        &self,
        prefer: ThemeTag,
        base_color: [u8; 3],
        frame: usize,
    ) -> Result<Vec<u8>> {
        let t = self.pick_theme(prefer, base_color);
        let px = self.pixels_per_frame();
        let scale = |v: u16| -> u8 {
            match self.color_depth {
                ColorDepth::Rgba8 => v as u8,
                ColorDepth::Rgba16 => (v >> 8) as u8,
            }
        };
        let pal = &self.palettes[t];
        let f = self
            .frames
            .get(frame)
            .ok_or(DifError::Invalid("frame index"))?;
        // Bake the depth-scaled RGBA8 palette once so the per-pixel loop is a copy.
        let lut: Vec<[u8; 4]> = pal
            .iter()
            .map(|c| [scale(c.r), scale(c.g), scale(c.b), scale(c.a)])
            .collect();
        let mut out = alloc::vec![0u8; px * 4];
        for (dst, &idx) in out.chunks_exact_mut(4).zip(&f.indices) {
            dst.copy_from_slice(&lut[idx as usize]);
        }
        Ok(out)
    }
}

/// Build a single-theme (light) indexed image from a packed RGBA8 buffer
/// (`4 * width * height` bytes, row-major). Dedups colors into a palette ordered
/// by **descending frequency** (ties by ascending packed key, for reproducible
/// bytes), so the hottest colors get the lowest indices. The image gets one
/// light-capable theme with a white base color; add a derived dark theme with
/// [`derive::derive_dark_palette`] afterwards.
///
/// `want` selects the index width: `None` auto-fits the smallest supported width
/// (quantizing when the source exceeds 16-bit), `Some(w)` forces width `w`
/// (quantizing down whenever the source has more colors than `w` can index). A
/// forced unsupported width (`Bit32`/`Bit64`) is an error.
///
/// Returns the image plus `Some(n)` when the palette was quantized — `n` is the
/// source's unique-color count before reduction — or `None` when it fit losslessly.
pub fn indexed_from_rgba8(
    width: u32,
    height: u32,
    rgba: &[u8],
    want: Option<IndexWidth>,
) -> Result<(DifImage, Option<u64>)> {
    #[cfg(not(feature = "std"))]
    use alloc::collections::BTreeMap as ColorMap;
    #[cfg(feature = "std")]
    use rustc_hash::FxHashMap as ColorMap;

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

    // Choose the target width: forced by `want`, else the smallest *supported*
    // width that fits — `for_count` may suggest an unsupported 32/64-bit width,
    // which we clamp to `Bit16` and quantize down into.
    let source = map.len() as u64;
    let index_width = match want {
        Some(w) => w,
        None => {
            let natural = IndexWidth::for_count(source);
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

    // Quantize between pass 1 and the reorder when the palette overflows the
    // target width: `quantize_oklab` rewrites `map` to representative keys ->
    // summed frequency and hands back `subst` (original key -> representative key)
    // for pass 2. `None` = the palette fit, so pass 2 maps keys straight through.
    // `source_colors` records the pre-quantization count only when reduced.
    #[cfg(feature = "derive")]
    let (subst, source_colors): (Option<ColorMap<u32, u32>>, Option<u64>) = if map.len() > capacity
    {
        (
            Some(quantize::quantize_oklab(&mut map, capacity)),
            Some(source),
        )
    } else {
        (None, None)
    };
    // Without `derive` there is no quantizer, so an overflowing palette is an error.
    #[cfg(not(feature = "derive"))]
    let (subst, source_colors): (Option<ColorMap<u32, u32>>, Option<u64>) = if map.len() > capacity
    {
        return Err(DifError::Invalid(
                "palette exceeds the index width capacity (build with the `derive` feature to quantize)",
            ));
    } else {
        (None, None)
    };

    // Order by frequency (desc), tie-break by key (asc) for determinism. After
    // quantization `map` holds the representative colors, so this orders those.
    let mut order: Vec<(u32, u32)> = map.iter().map(|(&k, &c)| (k, c)).collect();
    order.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // Materialize the palette and repurpose `map` as (representative) color -> index.
    let mut palette: Vec<Rgba> = Vec::with_capacity(order.len());
    for (idx, (key, _)) in order.iter().enumerate() {
        let b = key.to_le_bytes();
        palette.push(Rgba::new(
            b[0] as u16,
            b[1] as u16,
            b[2] as u16,
            b[3] as u16,
        ));
        map.insert(*key, idx as u32);
    }

    // Pass 2: emit the index frame. Each pixel's color resolves to its
    // representative (identity when not quantized), then to that color's index.
    let mut indices: Vec<u64> = Vec::with_capacity(px);
    for chunk in rgba.chunks_exact(4) {
        let key = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let rep = match &subst {
            Some(s) => s[&key],
            None => key,
        };
        indices.push(map[&rep] as u64);
    }

    let img = DifImage {
        width,
        height,
        color_depth: ColorDepth::Rgba8,
        index_width,
        themes: alloc::vec![Theme {
            abilities: abilities::LIGHT,
            base_color: [255, 255, 255],
        }],
        palettes: alloc::vec![palette],
        frames: alloc::vec![Frame {
            delay_us: 0,
            indices
        }],
        replay_count: 1,
    };
    img.validate()?;
    Ok((img, source_colors))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(img: &DifImage) -> (&Vec<Rgba>, &Vec<u64>) {
        (&img.palettes[0], &img.frames[0].indices)
    }

    #[test]
    fn palette_ordered_by_frequency_not_first_seen() {
        let a = [10u8, 20, 30, 255];
        let b = [200u8, 100, 50, 255];
        let rgba: Vec<u8> = [b, a, a, a].concat();
        let (img, quantized) = indexed_from_rgba8(2, 2, &rgba, None).unwrap();
        assert!(quantized.is_none(), "4 colors fit 8-bit without quantizing");
        let (palette, frame) = parts(&img);
        assert_eq!(
            palette[0],
            Rgba::new(10, 20, 30, 255),
            "hottest color first"
        );
        assert_eq!(palette[1], Rgba::new(200, 100, 50, 255));
        assert_eq!(frame, &alloc::vec![1u64, 0, 0, 0]);
        assert_eq!(img.index_width, IndexWidth::Bit8);
        assert_eq!(img.color_depth, ColorDepth::Rgba8);
    }

    #[test]
    fn pick_theme_by_capability_then_base_color() {
        let pal = alloc::vec![Rgba::new(0, 0, 0, 255)];
        let img = DifImage {
            width: 1,
            height: 1,
            color_depth: ColorDepth::Rgba8,
            index_width: IndexWidth::Bit8,
            themes: alloc::vec![
                Theme {
                    abilities: abilities::LIGHT,
                    base_color: [255, 255, 255]
                },
                Theme {
                    abilities: abilities::DARK,
                    base_color: [0, 0, 0]
                },
                Theme {
                    abilities: abilities::DARK,
                    base_color: [40, 40, 40]
                },
            ],
            palettes: alloc::vec![pal.clone(), pal.clone(), pal],
            frames: alloc::vec![Frame {
                delay_us: 0,
                indices: alloc::vec![0]
            }],
            replay_count: 1,
        };
        // Dark host with a near-black background -> theme 1 (base [0,0,0]) wins
        // over theme 2 (base [40,40,40]).
        assert_eq!(img.pick_theme(ThemeTag::Dark, [10, 10, 10]), 1);
        // Dark host with a charcoal background -> theme 2 is nearer.
        assert_eq!(img.pick_theme(ThemeTag::Dark, [45, 45, 45]), 2);
        // No high-contrast theme -> fall back to theme 0.
        assert_eq!(img.pick_theme(ThemeTag::HighContrast, [0, 0, 0]), 0);
    }

    #[test]
    fn for_count_suggests_smallest_holding_width() {
        assert_eq!(IndexWidth::for_count(256), IndexWidth::Bit8);
        assert_eq!(IndexWidth::for_count(257), IndexWidth::Bit16);
        assert_eq!(IndexWidth::for_count(65536), IndexWidth::Bit16);
        // Beyond 16-bit it suggests an *unsupported* width rather than erroring;
        // the encoder reads that as "must quantize".
        assert_eq!(IndexWidth::for_count(65537), IndexWidth::Bit32);
        assert!(!IndexWidth::for_count(65537).supported());
        assert_eq!(IndexWidth::for_count((1 << 32) + 1), IndexWidth::Bit64);
    }

    #[test]
    fn index_width_table_round_trips_all_four() {
        for (w, bits, bytes) in [
            (IndexWidth::Bit8, 0b00u8, 1usize),
            (IndexWidth::Bit16, 0b01, 2),
            (IndexWidth::Bit32, 0b10, 4),
            (IndexWidth::Bit64, 0b11, 8),
        ] {
            assert_eq!(w.to_bits(), bits);
            assert_eq!(IndexWidth::from_bits(bits).unwrap(), w);
            assert_eq!(w.bytes(), bytes);
        }
        assert!(IndexWidth::Bit8.supported() && IndexWidth::Bit16.supported());
        assert!(!IndexWidth::Bit32.supported() && !IndexWidth::Bit64.supported());
    }

    #[test]
    fn forced_unsupported_width_errors() {
        let rgba = [0u8; 16]; // 2x2, one color
        assert!(matches!(
            indexed_from_rgba8(2, 2, &rgba, Some(IndexWidth::Bit32)),
            Err(DifError::BadIndexWidth(32))
        ));
    }

    /// Build an `(side*side)`-pixel image whose every pixel is a distinct color
    /// (so unique-color count == pixel count), with `a` cycling so alpha varies.
    #[cfg(test)]
    fn distinct_colors(side: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((side * side * 4) as usize);
        for i in 0..(side * side) {
            rgba.extend_from_slice(&[
                (i & 0xff) as u8,
                ((i >> 8) & 0xff) as u8,
                ((i >> 4) & 0xff) as u8,
                (200 + (i % 56)) as u8,
            ]);
        }
        rgba
    }

    #[cfg(feature = "derive")]
    #[test]
    fn quantize_forced_8bit_reduces_deterministically() {
        let side = 20; // 400 distinct colors -> must fold into <= 256
        let rgba = distinct_colors(side);
        let (img, q) = indexed_from_rgba8(side, side, &rgba, Some(IndexWidth::Bit8)).unwrap();
        assert_eq!(img.index_width, IndexWidth::Bit8);
        assert!(img.palettes[0].len() <= 256, "palette must fit 8-bit");
        assert_eq!(q, Some(400), "reports pre-quantization color count");
        // Every emitted index is in range (also asserted by `validate`).
        assert!(img.frames[0]
            .indices
            .iter()
            .all(|&i| (i as usize) < img.palettes[0].len()));
        // Deterministic: same bytes + same metadata on a second run.
        let (img2, q2) = indexed_from_rgba8(side, side, &rgba, Some(IndexWidth::Bit8)).unwrap();
        assert_eq!(to_difr(&img).unwrap(), to_difr(&img2).unwrap());
        assert_eq!(q, q2);
    }

    #[cfg(feature = "derive")]
    #[test]
    fn forced_16bit_keeps_all_colors_without_quantizing() {
        let side = 20; // 400 distinct colors all fit 16-bit
        let rgba = distinct_colors(side);
        let (img, q) = indexed_from_rgba8(side, side, &rgba, Some(IndexWidth::Bit16)).unwrap();
        assert_eq!(img.index_width, IndexWidth::Bit16);
        assert_eq!(img.palettes[0].len(), 400);
        assert_eq!(q, None, "fit losslessly, so not quantized");
    }
}
