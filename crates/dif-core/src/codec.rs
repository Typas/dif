//! Compressed `.dif` container: a small header naming the codec and the level
//! that produced it, then the losslessly compressed `.difr` body.
//!
//! ```text
//! magic:"DIF1"  version:u8  codec:u8  level:u8  raw_len:u64   compressed_body:[u8]
//! ```
//!
//! The `level` byte records *which level* of the codec family produced the body
//! (e.g. `zstd-3` vs `zstd-10`). It is informational/forward-compatible: every
//! supported codec's compressed stream is self-describing, so `decompress`
//! reads the level from the header but does **not** pass it to the decoder.
//!
//! Codecs should prefer a pure-Rust implementation so the decoder compiles to
//! wasm; native-only codecs (e.g. `Zstd`, `Lzav`) are allowed but unavailable in
//! the portable wasm decoder. The benchmark studies many more codecs over the
//! `.difr` body.

use alloc::vec::Vec;

use crate::error::{DifError, Result};
use crate::format::{self, VERSION};
use crate::DifImage;

const MAGIC_DIF: [u8; 4] = *b"DIF1";

/// Compression algorithm used for a `.dif` body.
///
/// `Store`/`Deflate`/`Lz4` are pure-Rust and decode in the default no_std build.
/// `Brotli` requires the `std` feature; `Zstd` and `Lzav` require a C-codec
/// feature (`native` on the host, or `wasm-native` for the zig-cross wasm
/// decoder) and are unavailable in a plain no_std build. Byte 3 is reserved
/// (formerly `Xz`, removed) and rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum CodecId {
    /// No compression — body stored verbatim.
    Store = 0,
    /// DEFLATE via `miniz_oxide` (decode); `libdeflater` encoder under `native`.
    Deflate = 1,
    /// Brotli (`brotli` crate). Requires the `std` feature.
    Brotli = 2,
    // 3 = reserved (was Xz, removed).
    /// Zstandard via `zstd-safe`. Requires the `native` feature.
    Zstd = 4,
    /// LZ4 block via `lz4_flex` (pure-Rust, portable + wasm-decodable).
    Lz4 = 5,
    /// LZAV via the vendored `lzav-shim` C shim. Requires the `native` feature.
    Lzav = 6,
}

impl CodecId {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(CodecId::Store),
            1 => Ok(CodecId::Deflate),
            2 => Ok(CodecId::Brotli),
            4 => Ok(CodecId::Zstd),
            5 => Ok(CodecId::Lz4),
            6 => Ok(CodecId::Lzav),
            _ => Err(DifError::BadCodec(v)),
        }
    }
}

/// Compress `data` with `codec` at `level`. The per-family meaning of `level`:
/// Deflate 0–9 (libdeflate 1–12), Brotli quality 0–11, Zstd 1–22, Lz4/Lzav
/// ignore it (only their fast level is wired). `Store` ignores it too.
fn compress(data: &[u8], codec: CodecId, level: u8) -> Result<Vec<u8>> {
    match codec {
        CodecId::Store => Ok(data.to_vec()),
        CodecId::Deflate => deflate_compress(data, level),
        CodecId::Brotli => brotli_compress(data, level),
        CodecId::Zstd => zstd_compress(data, level),
        CodecId::Lz4 => Ok(lz4_flex::block::compress(data)),
        CodecId::Lzav => lzav_compress(data),
    }
}

/// Decompress `data` (known uncompressed size `raw_len`). The header's level
/// byte is not needed — every codec's stream is self-describing.
fn decompress(codec: CodecId, data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    match codec {
        CodecId::Store => Ok(data.to_vec()),
        CodecId::Deflate => {
            miniz_oxide::inflate::decompress_to_vec(data).map_err(|_| DifError::CompressionFailed)
        }
        CodecId::Brotli => brotli_decompress(data, raw_len),
        CodecId::Zstd => zstd_decompress(data, raw_len),
        CodecId::Lz4 => {
            lz4_flex::block::decompress(data, raw_len).map_err(|_| DifError::CompressionFailed)
        }
        CodecId::Lzav => lzav_decompress(data, raw_len),
    }
}

// --- Deflate: miniz_oxide encoder by default; libdeflate encoder under `native`.
//     Both emit a standard raw DEFLATE stream, decoded by miniz_oxide everywhere.

#[cfg(not(feature = "native"))]
fn deflate_compress(data: &[u8], level: u8) -> Result<Vec<u8>> {
    Ok(miniz_oxide::deflate::compress_to_vec(data, level))
}

#[cfg(feature = "native")]
fn deflate_compress(data: &[u8], level: u8) -> Result<Vec<u8>> {
    use libdeflater::{CompressionLvl, Compressor};
    let lvl = CompressionLvl::new(level as i32).map_err(|_| DifError::CompressionFailed)?;
    let mut c = Compressor::new(lvl);
    let mut out = vec![0u8; c.deflate_compress_bound(data.len())];
    let n = c
        .deflate_compress(data, &mut out)
        .map_err(|_| DifError::CompressionFailed)?;
    out.truncate(n);
    Ok(out)
}

// --- Lzav: C shim (native or zig-cross wasm). `lzav-1` is the single level. ---

#[cfg(feature = "c-codecs")]
fn lzav_compress(data: &[u8]) -> Result<Vec<u8>> {
    lzav_shim::compress(data).ok_or(DifError::CompressionFailed)
}

#[cfg(feature = "c-codecs")]
fn lzav_decompress(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    lzav_shim::decompress(data, raw_len).ok_or(DifError::CompressionFailed)
}

#[cfg(not(feature = "c-codecs"))]
fn lzav_compress(_data: &[u8]) -> Result<Vec<u8>> {
    Err(DifError::Invalid("lzav codec requires a C-codec feature"))
}

#[cfg(not(feature = "c-codecs"))]
fn lzav_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("lzav codec requires a C-codec feature"))
}

// --- Brotli: std-only (the `brotli` crate's streaming Reader/Writer need std) ---

#[cfg(feature = "std")]
fn brotli_compress(data: &[u8], level: u8) -> Result<Vec<u8>> {
    use std::io::Write as _;
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 4096, level as u32, 22);
        w.write_all(data).map_err(|_| DifError::CompressionFailed)?;
    }
    Ok(out)
}

#[cfg(feature = "std")]
fn brotli_decompress(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::with_capacity(raw_len);
    let mut r = brotli::Decompressor::new(data, 4096);
    r.read_to_end(&mut out)
        .map_err(|_| DifError::CompressionFailed)?;
    Ok(out)
}

#[cfg(not(feature = "std"))]
fn brotli_compress(_data: &[u8], _level: u8) -> Result<Vec<u8>> {
    Err(DifError::Invalid("brotli codec requires the `std` feature"))
}

#[cfg(not(feature = "std"))]
fn brotli_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("brotli codec requires the `std` feature"))
}

// --- Zstd: with the C-codec feature set (zstd-safe links C zstd) ---

#[cfg(feature = "c-codecs")]
fn zstd_compress(data: &[u8], level: u8) -> Result<Vec<u8>> {
    let mut out = vec![0u8; zstd_safe::compress_bound(data.len())];
    let n = zstd_safe::compress(out.as_mut_slice(), data, level as i32)
        .map_err(|_| DifError::CompressionFailed)?;
    out.truncate(n);
    Ok(out)
}

#[cfg(feature = "c-codecs")]
fn zstd_decompress(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; raw_len];
    let n =
        zstd_safe::decompress(out.as_mut_slice(), data).map_err(|_| DifError::CompressionFailed)?;
    out.truncate(n);
    Ok(out)
}

#[cfg(not(feature = "c-codecs"))]
fn zstd_compress(_data: &[u8], _level: u8) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zstd codec requires a C-codec feature"))
}

#[cfg(not(feature = "c-codecs"))]
fn zstd_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zstd codec requires a C-codec feature"))
}

/// Serialize and compress an image into a `.dif` container at codec `level`.
/// The level is recorded in the header for provenance; decode ignores it.
pub fn to_dif(img: &DifImage, codec: CodecId, level: u8) -> Result<Vec<u8>> {
    img.validate()?;
    let mut body = Vec::new();
    format::write_body(img, &mut body);
    let compressed = compress(&body, codec, level)?;

    let mut out = Vec::with_capacity(compressed.len() + 15);
    out.extend_from_slice(&MAGIC_DIF);
    out.push(VERSION);
    out.push(codec as u8);
    out.push(level);
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

/// Parse and decompress a `.dif` container.
pub fn from_dif(bytes: &[u8]) -> Result<DifImage> {
    if bytes.len() < 15 {
        return Err(DifError::UnexpectedEof);
    }
    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if magic != MAGIC_DIF {
        return Err(DifError::BadMagic(magic));
    }
    if bytes[4] != VERSION {
        return Err(DifError::BadVersion(bytes[4]));
    }
    let codec = CodecId::from_u8(bytes[5])?;
    // bytes[6] = level: self-describing streams don't need it on decode.
    let raw_len = u64::from_le_bytes(bytes[7..15].try_into().unwrap()) as usize;
    let body = decompress(codec, &bytes[15..], raw_len)?;
    if body.len() != raw_len {
        return Err(DifError::Invalid("decompressed length mismatch"));
    }
    let mut pos = 0;
    format::read_body(&body, &mut pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Content, ModeTag, Rgba, SampleDepth, Theme};

    fn sample() -> DifImage {
        let light = vec![Rgba::new(255, 255, 255, 255), Rgba::new(0, 0, 0, 255)];
        let dark = vec![Rgba::new(0, 0, 0, 255), Rgba::new(255, 255, 255, 255)];
        DifImage {
            width: 4,
            height: 4,
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
                frames: vec![vec![0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 1]],
            },
            frame_delays: vec![0],
        }
    }

    #[test]
    fn dif_roundtrip_all_codecs() {
        let img = sample();
        // (codec, level). `mut`/pushes are feature-gated; with no features only
        // the portable set (Store/Deflate/Lz4) exists.
        #[allow(unused_mut)]
        let mut codecs = vec![
            (CodecId::Store, 0u8),
            (CodecId::Deflate, 6),
            (CodecId::Lz4, 1),
        ];
        #[cfg(feature = "std")]
        codecs.push((CodecId::Brotli, 5));
        #[cfg(feature = "native")]
        {
            codecs.push((CodecId::Zstd, 3));
            codecs.push((CodecId::Lzav, 1));
        }
        for (codec, level) in codecs {
            let bytes = to_dif(&img, codec, level).unwrap();
            assert_eq!(&bytes[..4], b"DIF1");
            assert_eq!(bytes[5], codec as u8);
            assert_eq!(bytes[6], level, "level byte for {codec:?}");
            let back = from_dif(&bytes).unwrap();
            assert_eq!(img, back, "codec {codec:?}");
        }
    }

    #[test]
    #[cfg(feature = "native")]
    fn zstd_level_roundtrip() {
        // Different levels produce different bodies but both decode equal — the
        // level byte is recorded yet not consumed by decode.
        let img = sample();
        let lo = to_dif(&img, CodecId::Zstd, 3).unwrap();
        let hi = to_dif(&img, CodecId::Zstd, 10).unwrap();
        assert_eq!(lo[6], 3);
        assert_eq!(hi[6], 10);
        assert_eq!(from_dif(&lo).unwrap(), img);
        assert_eq!(from_dif(&hi).unwrap(), img);
    }

    #[test]
    #[cfg(feature = "std")]
    fn brotli_smaller_than_store_on_repetitive() {
        // A large repetitive image should compress well.
        let frame = vec![0u32; 64 * 64];
        let pal = vec![Rgba::new(1, 2, 3, 255)];
        let img = DifImage {
            width: 64,
            height: 64,
            depth: SampleDepth::Eight,
            themes: vec![Theme {
                tag: ModeTag::Light,
                name: "l".into(),
            }],
            content: Content::Indexed {
                palettes: vec![pal],
                frames: vec![frame],
            },
            frame_delays: vec![0],
        };
        let store = to_dif(&img, CodecId::Store, 0).unwrap().len();
        let brotli = to_dif(&img, CodecId::Brotli, 5).unwrap().len();
        assert!(brotli < store, "brotli {brotli} should beat store {store}");
    }

    #[test]
    fn bad_codec_byte_rejected() {
        let mut bytes = to_dif(&sample(), CodecId::Store, 0).unwrap();
        bytes[5] = 99;
        assert!(matches!(from_dif(&bytes), Err(DifError::BadCodec(99))));
    }

    #[test]
    fn reserved_codec_byte_3_rejected() {
        // Byte 3 was Xz; it must now be rejected as unknown.
        let mut bytes = to_dif(&sample(), CodecId::Store, 0).unwrap();
        bytes[5] = 3;
        assert!(matches!(from_dif(&bytes), Err(DifError::BadCodec(3))));
    }
}
