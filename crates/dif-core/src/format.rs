//! DIF v3 byte layout: the fixed 64-byte container header and the pure
//! (de)serialization of the *decompressed* body sections (themes, palettes, and
//! per-frame index planes). This module is codec-agnostic --- [`crate::codec`]
//! drives the two-stage compression and assembles the intermediate body.
//!
//! Header (64 bytes, little-endian):
//!
//! ```text
//! 0  magic[8]   "DIF3\0\0\0\0" | "DIFR3\0\0\0"   (carries version '3')
//! 8  codec:u8         outer whole-body  (4b family | 4b level index)
//! 9  flags:u8         bit0-1 index width; bit2-5 color depth
//! 10 codec_palette:u8 per-palette section codec
//! 11 codec_frame:u8   per-frame section codec
//! 12 theme_count:u8   stored = count - 1  (1..=256)
//! 13 reserved:u8
//! 14 frame_count:u16
//! 16 replay_count:u16 0=infinite, 1=static
//! 18 reserved:u16
//! 20 width:u32
//! 24 height:u32
//! 28 frame_long_offset:u32  upper 32 bits of the first-frame offset
//! 32 frame_offset:u64       lower 64 bits of the first-frame offset
//! 40 frame_alignment:u64    per-frame stride (multiple of 16)
//! 48 index_count:u64        palette length (color count)
//! 56 palette_size:u64       compressed palette-section length (was reserved)
//! 64 compressed_body[]
//! ```
//!
//! `palette_size` (offset 56, a v2-reserved slot) records the exact byte length of
//! the single compressed palette blob so the decoder can bound it without relying
//! on a self-terminating codec; the palette still begins at the spec offset
//! `64 + align(4*theme_count, 16)`.

use alloc::vec::Vec;

use crate::error::{DifError, Result};
use crate::{ColorDepth, IndexWidth, Rgba, Theme};

pub(crate) const MAGIC_DIF: [u8; 8] = *b"DIF3\0\0\0\0";
pub(crate) const MAGIC_DIFR: [u8; 8] = *b"DIFR3\0\0\0";
pub(crate) const HEADER_LEN: usize = 64;

/// Round `n` up to the next multiple of 16.
pub(crate) const fn align16(n: usize) -> usize {
    (n + 15) & !15
}

/// Parsed 64-byte container header.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Header {
    pub is_raw: bool,
    pub codec: u8,
    pub flags: u8,
    pub codec_palette: u8,
    pub codec_frame: u8,
    pub theme_count: usize,
    pub frame_count: usize,
    pub replay_count: u16,
    pub width: u32,
    pub height: u32,
    /// First-frame offset within the (outer-decompressed) intermediate body,
    /// measured from the file start: `long * 2^64 + offset`.
    pub first_frame_offset: u128,
    pub frame_alignment: u64,
    pub index_count: u64,
    /// Compressed length of the single palette blob (offset 56).
    pub palette_size: u64,
}

impl Header {
    pub fn index_width(&self) -> Result<IndexWidth> {
        IndexWidth::from_bits(self.flags & 0b11)
    }
    pub fn color_depth(&self) -> Result<ColorDepth> {
        ColorDepth::from_bits((self.flags >> 2) & 0xF)
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        let magic = if self.is_raw { MAGIC_DIFR } else { MAGIC_DIF };
        out.extend_from_slice(&magic);
        out.push(self.codec);
        out.push(self.flags);
        out.push(self.codec_palette);
        out.push(self.codec_frame);
        out.push((self.theme_count - 1) as u8);
        out.push(0); // reserved
        out.extend_from_slice(&(self.frame_count as u16).to_le_bytes());
        out.extend_from_slice(&self.replay_count.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        let long = (self.first_frame_offset >> 64) as u32;
        let low = self.first_frame_offset as u64;
        out.extend_from_slice(&long.to_le_bytes());
        out.extend_from_slice(&low.to_le_bytes());
        out.extend_from_slice(&self.frame_alignment.to_le_bytes());
        out.extend_from_slice(&self.index_count.to_le_bytes());
        out.extend_from_slice(&self.palette_size.to_le_bytes());
        debug_assert_eq!(out.len() % HEADER_LEN, 0);
    }

    pub fn read(bytes: &[u8]) -> Result<Header> {
        if bytes.len() < HEADER_LEN {
            return Err(DifError::UnexpectedEof);
        }
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&bytes[0..8]);
        let is_raw = if magic == MAGIC_DIF {
            false
        } else if magic == MAGIC_DIFR {
            true
        } else {
            return Err(DifError::BadMagic(magic));
        };
        let theme_count = bytes[12] as usize + 1;
        let frame_count = u16::from_le_bytes([bytes[14], bytes[15]]) as usize;
        let replay_count = u16::from_le_bytes([bytes[16], bytes[17]]);
        let width = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let height = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let long = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let low = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        let frame_alignment = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
        let index_count = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
        let palette_size = u64::from_le_bytes(bytes[56..64].try_into().unwrap());
        Ok(Header {
            is_raw,
            codec: bytes[8],
            flags: bytes[9],
            codec_palette: bytes[10],
            codec_frame: bytes[11],
            theme_count,
            frame_count,
            replay_count,
            width,
            height,
            first_frame_offset: ((long as u128) << 64) | (low as u128),
            frame_alignment,
            index_count,
            palette_size,
        })
    }
}

// --- pure section (de)serialization (the decompressed forms) --------------

/// `theme_count * 4` bytes: `abilities, r, g, b` per theme.
pub(crate) fn themes_bytes(themes: &[Theme], out: &mut Vec<u8>) {
    for t in themes {
        out.push(t.abilities);
        out.push(t.base_color[0]);
        out.push(t.base_color[1]);
        out.push(t.base_color[2]);
    }
}

pub(crate) fn read_themes(bytes: &[u8], count: usize) -> Result<Vec<Theme>> {
    if bytes.len() < count * 4 {
        return Err(DifError::UnexpectedEof);
    }
    let mut themes = Vec::with_capacity(count);
    for i in 0..count {
        let b = &bytes[i * 4..i * 4 + 4];
        themes.push(Theme {
            abilities: b[0],
            base_color: [b[1], b[2], b[3]],
        });
    }
    Ok(themes)
}

/// All palettes concatenated: `palettes[theme][index]` as RGBA at `depth`.
pub(crate) fn palettes_bytes(palettes: &[Vec<Rgba>], depth: ColorDepth, out: &mut Vec<u8>) {
    for pal in palettes {
        for c in pal {
            write_channel(out, c.r, depth);
            write_channel(out, c.g, depth);
            write_channel(out, c.b, depth);
            write_channel(out, c.a, depth);
        }
    }
}

pub(crate) fn read_palettes(
    bytes: &[u8],
    theme_count: usize,
    index_count: usize,
    depth: ColorDepth,
) -> Result<Vec<Vec<Rgba>>> {
    let need = theme_count * index_count * depth.color_bytes();
    if bytes.len() < need {
        return Err(DifError::UnexpectedEof);
    }
    let mut pos = 0;
    let mut palettes = Vec::with_capacity(theme_count);
    for _ in 0..theme_count {
        let mut pal = Vec::with_capacity(index_count);
        for _ in 0..index_count {
            let r = read_channel(bytes, &mut pos, depth);
            let g = read_channel(bytes, &mut pos, depth);
            let b = read_channel(bytes, &mut pos, depth);
            let a = read_channel(bytes, &mut pos, depth);
            pal.push(Rgba::new(r, g, b, a));
        }
        palettes.push(pal);
    }
    Ok(palettes)
}

/// One frame's index plane: `width*height` indices at `width`.
pub(crate) fn frame_bitmap_bytes(indices: &[u64], width: IndexWidth, out: &mut Vec<u8>) {
    match width {
        IndexWidth::Bit8 => out.extend(indices.iter().map(|&i| i as u8)),
        IndexWidth::Bit16 => {
            for &i in indices {
                out.extend_from_slice(&(i as u16).to_le_bytes());
            }
        }
        // Serialization is only reached after `validate` accepts the width.
        IndexWidth::Bit32 | IndexWidth::Bit64 => {
            unreachable!("unsupported index width reaches serialization")
        }
    }
}

pub(crate) fn read_frame_bitmap(bytes: &[u8], px: usize, width: IndexWidth) -> Result<Vec<u64>> {
    let need = px * width.bytes();
    if bytes.len() < need {
        return Err(DifError::UnexpectedEof);
    }
    let mut out = Vec::with_capacity(px);
    match width {
        IndexWidth::Bit8 => out.extend(bytes[..px].iter().map(|&b| b as u64)),
        IndexWidth::Bit16 => {
            for c in bytes[..px * 2].chunks_exact(2) {
                out.push(u16::from_le_bytes([c[0], c[1]]) as u64);
            }
        }
        // An unsupported width forged into the flags: reject with its bit count.
        IndexWidth::Bit32 | IndexWidth::Bit64 => {
            return Err(DifError::BadIndexWidth((width.bytes() * 8) as u8));
        }
    }
    Ok(out)
}

fn write_channel(out: &mut Vec<u8>, v: u16, depth: ColorDepth) {
    match depth {
        ColorDepth::Rgba8 => out.push(v as u8),
        ColorDepth::Rgba16 => out.extend_from_slice(&v.to_le_bytes()),
    }
}

fn read_channel(bytes: &[u8], pos: &mut usize, depth: ColorDepth) -> u16 {
    match depth {
        ColorDepth::Rgba8 => {
            let v = bytes[*pos] as u16;
            *pos += 1;
            v
        }
        ColorDepth::Rgba16 => {
            let v = u16::from_le_bytes([bytes[*pos], bytes[*pos + 1]]);
            *pos += 2;
            v
        }
    }
}
