//! Raw (uncompressed) serialization of a [`DifImage`] — the `.difr` body.
//!
//! Layout (little-endian), used both standalone (`.difr`, magic `DIFR`) and as
//! the payload inside a compressed `.dif` container:
//!
//! ```text
//! flags:u8   bit0 mode(0=indexed,1=grayscale)  bit1 depth(0=8bit,1=16bit)
//! width:u32  height:u32  frame_count:u32  theme_count:u8
//! themes[theme_count]:  tag:u8  name_len:u8  name:[u8; name_len]
//! frame_delays[frame_count]: u16
//! indexed:   color_count:varint
//!            palette[theme][color]: RGBA, each channel 1 or 2 bytes
//!            frames[frame][pixel]: varint index
//! grayscale: lut[theme][level]: sample (1 or 2 bytes), level in 0..depth.levels()
//!            frames[frame][pixel]: sample (1 or 2 bytes)
//! ```

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::{DifError, Result};
use crate::{varint, Content, DifImage, ModeTag, Rgba, SampleDepth, Theme};

pub(crate) const MAGIC_RAW: [u8; 4] = *b"DIFR";
pub(crate) const VERSION: u8 = 1;

const FLAG_GRAYSCALE: u8 = 1 << 0;
const FLAG_DEPTH16: u8 = 1 << 1;

/// Serialize an image to a standalone raw `.difr` byte vector.
pub fn to_difr(img: &DifImage) -> Result<Vec<u8>> {
    img.validate()?;
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC_RAW);
    out.push(VERSION);
    write_body(img, &mut out);
    Ok(out)
}

/// Parse a standalone raw `.difr` byte slice.
pub fn from_difr(bytes: &[u8]) -> Result<DifImage> {
    let mut pos = 0;
    let magic = read_array4(bytes, &mut pos)?;
    if magic != MAGIC_RAW {
        return Err(DifError::BadMagic(magic));
    }
    let version = read_u8(bytes, &mut pos)?;
    if version != VERSION {
        return Err(DifError::BadVersion(version));
    }
    read_body(bytes, &mut pos)
}

// --- body (shared with the compressed container) -------------------------

pub(crate) fn write_body(img: &DifImage, out: &mut Vec<u8>) {
    let mut flags = 0u8;
    if matches!(img.content, Content::Grayscale { .. }) {
        flags |= FLAG_GRAYSCALE;
    }
    if img.depth == SampleDepth::Sixteen {
        flags |= FLAG_DEPTH16;
    }
    out.push(flags);
    out.extend_from_slice(&img.width.to_le_bytes());
    out.extend_from_slice(&img.height.to_le_bytes());
    let frame_count = img.frame_count() as u32;
    out.extend_from_slice(&frame_count.to_le_bytes());
    out.push(img.themes.len() as u8);

    for t in &img.themes {
        out.push(t.tag as u8);
        let name = t.name.as_bytes();
        out.push(name.len() as u8);
        out.extend_from_slice(name);
    }

    // Per-frame delays (pad with zeros if the caller left them short).
    for f in 0..frame_count as usize {
        let d = img.frame_delays.get(f).copied().unwrap_or(0);
        out.extend_from_slice(&d.to_le_bytes());
    }

    let depth = img.depth;
    match &img.content {
        Content::Indexed { palettes, frames } => {
            let color_count = palettes[0].len() as u32;
            varint::write(out, color_count);
            for pal in palettes {
                for c in pal {
                    write_rgba(out, *c, depth);
                }
            }
            for f in frames {
                for &idx in f {
                    varint::write(out, idx);
                }
            }
        }
        Content::Grayscale { luts, frames } => {
            for lut in luts {
                for &v in lut {
                    write_sample(out, v, depth);
                }
            }
            for f in frames {
                for &s in f {
                    write_sample(out, s, depth);
                }
            }
        }
    }
}

pub(crate) fn read_body(bytes: &[u8], pos: &mut usize) -> Result<DifImage> {
    let flags = read_u8(bytes, pos)?;
    let grayscale = flags & FLAG_GRAYSCALE != 0;
    let depth = if flags & FLAG_DEPTH16 != 0 {
        SampleDepth::Sixteen
    } else {
        SampleDepth::Eight
    };
    let width = read_u32(bytes, pos)?;
    let height = read_u32(bytes, pos)?;
    let frame_count = read_u32(bytes, pos)? as usize;
    let theme_count = read_u8(bytes, pos)? as usize;
    if theme_count == 0 || theme_count > 128 {
        return Err(DifError::BadThemeCount(theme_count));
    }

    let mut themes = Vec::with_capacity(theme_count);
    for _ in 0..theme_count {
        let tag = ModeTag::from_u8(read_u8(bytes, pos)?)?;
        let name_len = read_u8(bytes, pos)? as usize;
        let name_bytes = read_bytes(bytes, pos, name_len)?;
        let name = String::from_utf8(name_bytes.to_vec())
            .map_err(|_| DifError::Invalid("theme name not UTF-8"))?;
        themes.push(Theme { tag, name });
    }

    let mut frame_delays = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        frame_delays.push(read_u16(bytes, pos)?);
    }

    let px = width as usize * height as usize;
    let content = if grayscale {
        let levels = depth.levels();
        let mut luts = Vec::with_capacity(theme_count);
        for _ in 0..theme_count {
            let mut lut = Vec::with_capacity(levels);
            for _ in 0..levels {
                lut.push(read_sample(bytes, pos, depth)?);
            }
            luts.push(lut);
        }
        let mut frames = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let mut f = Vec::with_capacity(px);
            for _ in 0..px {
                f.push(read_sample(bytes, pos, depth)?);
            }
            frames.push(f);
        }
        Content::Grayscale { luts, frames }
    } else {
        let color_count = varint::read(bytes, pos)? as usize;
        let mut palettes = Vec::with_capacity(theme_count);
        for _ in 0..theme_count {
            let mut pal = Vec::with_capacity(color_count);
            for _ in 0..color_count {
                pal.push(read_rgba(bytes, pos, depth)?);
            }
            palettes.push(pal);
        }
        let mut frames = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let mut f = Vec::with_capacity(px);
            for _ in 0..px {
                f.push(varint::read(bytes, pos)?);
            }
            frames.push(f);
        }
        Content::Indexed { palettes, frames }
    };

    let img = DifImage {
        width,
        height,
        depth,
        themes,
        content,
        frame_delays,
    };
    img.validate()?;
    Ok(img)
}

// --- primitive readers/writers -------------------------------------------

fn write_sample(out: &mut Vec<u8>, v: u16, depth: SampleDepth) {
    match depth {
        SampleDepth::Eight => out.push(v as u8),
        SampleDepth::Sixteen => out.extend_from_slice(&v.to_le_bytes()),
    }
}

fn write_rgba(out: &mut Vec<u8>, c: Rgba, depth: SampleDepth) {
    write_sample(out, c.r, depth);
    write_sample(out, c.g, depth);
    write_sample(out, c.b, depth);
    write_sample(out, c.a, depth);
}

fn read_sample(bytes: &[u8], pos: &mut usize, depth: SampleDepth) -> Result<u16> {
    match depth {
        SampleDepth::Eight => Ok(read_u8(bytes, pos)? as u16),
        SampleDepth::Sixteen => read_u16(bytes, pos),
    }
}

fn read_rgba(bytes: &[u8], pos: &mut usize, depth: SampleDepth) -> Result<Rgba> {
    Ok(Rgba {
        r: read_sample(bytes, pos, depth)?,
        g: read_sample(bytes, pos, depth)?,
        b: read_sample(bytes, pos, depth)?,
        a: read_sample(bytes, pos, depth)?,
    })
}

fn read_u8(bytes: &[u8], pos: &mut usize) -> Result<u8> {
    let v = *bytes.get(*pos).ok_or(DifError::UnexpectedEof)?;
    *pos += 1;
    Ok(v)
}

fn read_u16(bytes: &[u8], pos: &mut usize) -> Result<u16> {
    let s = read_bytes(bytes, pos, 2)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32> {
    let s = read_bytes(bytes, pos, 4)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_array4(bytes: &[u8], pos: &mut usize) -> Result<[u8; 4]> {
    let s = read_bytes(bytes, pos, 4)?;
    Ok([s[0], s[1], s[2], s[3]])
}

fn read_bytes<'a>(bytes: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8]> {
    let end = pos.checked_add(n).ok_or(DifError::UnexpectedEof)?;
    let s = bytes.get(*pos..end).ok_or(DifError::UnexpectedEof)?;
    *pos = end;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indexed_2x2_two_themes() -> DifImage {
        // light: black on white; dark: white on black.
        let light = vec![Rgba::new(255, 255, 255, 255), Rgba::new(0, 0, 0, 255)];
        let dark = vec![Rgba::new(0, 0, 0, 255), Rgba::new(255, 255, 255, 255)];
        DifImage {
            width: 2,
            height: 2,
            depth: SampleDepth::Eight,
            themes: vec![
                Theme {
                    tag: ModeTag::Light,
                    name: "light".into(),
                },
                Theme {
                    tag: ModeTag::Dark,
                    name: "dark".into(),
                },
            ],
            content: Content::Indexed {
                palettes: vec![light, dark],
                frames: vec![vec![0, 1, 1, 0]],
            },
            frame_delays: vec![0],
        }
    }

    fn grayscale_2x2() -> DifImage {
        let levels = SampleDepth::Eight.levels();
        let identity: Vec<u16> = (0..levels as u16).collect();
        let inverted: Vec<u16> = (0..levels as u16).map(|v| 255 - v).collect();
        DifImage {
            width: 2,
            height: 2,
            depth: SampleDepth::Eight,
            themes: vec![
                Theme {
                    tag: ModeTag::Light,
                    name: "light".into(),
                },
                Theme {
                    tag: ModeTag::Dark,
                    name: "dark".into(),
                },
            ],
            content: Content::Grayscale {
                luts: vec![identity, inverted],
                frames: vec![vec![10, 200, 0, 255]],
            },
            frame_delays: vec![0],
        }
    }

    #[test]
    fn difr_roundtrip_indexed() {
        let img = indexed_2x2_two_themes();
        let bytes = to_difr(&img).unwrap();
        assert_eq!(&bytes[..4], b"DIFR");
        let back = from_difr(&bytes).unwrap();
        assert_eq!(img, back);
    }

    #[test]
    fn difr_roundtrip_grayscale() {
        let img = grayscale_2x2();
        let back = from_difr(&to_difr(&img).unwrap()).unwrap();
        assert_eq!(img, back);
    }

    #[test]
    fn difr_roundtrip_16bit_indexed() {
        let mut img = indexed_2x2_two_themes();
        img.depth = SampleDepth::Sixteen;
        if let Content::Indexed { palettes, .. } = &mut img.content {
            palettes[0][0] = Rgba::new(65535, 1000, 0, 65535);
        }
        let back = from_difr(&to_difr(&img).unwrap()).unwrap();
        assert_eq!(img, back);
    }

    #[test]
    fn render_picks_dark_theme() {
        let img = indexed_2x2_two_themes();
        // frame = [0,1,1,0]; dark palette: idx0=black, idx1=white.
        let rgba = img.render_rgba8(ModeTag::Dark, 0).unwrap();
        assert_eq!(&rgba[0..4], &[0, 0, 0, 255]); // pixel 0 -> idx0 -> black
        assert_eq!(&rgba[4..8], &[255, 255, 255, 255]); // pixel 1 -> idx1 -> white
    }

    #[test]
    fn render_grayscale_inverts_on_dark() {
        let img = grayscale_2x2();
        let light = img.render_rgba8(ModeTag::Light, 0).unwrap();
        let dark = img.render_rgba8(ModeTag::Dark, 0).unwrap();
        assert_eq!(light[0], 10); // identity
        assert_eq!(dark[0], 245); // 255 - 10
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = to_difr(&indexed_2x2_two_themes()).unwrap();
        bytes[0] = b'X';
        assert!(matches!(from_difr(&bytes), Err(DifError::BadMagic(_))));
    }
}
