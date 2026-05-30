//! Compressed `.dif` container: a small header naming the codec, then the
//! losslessly compressed `.difr` body.
//!
//! ```text
//! magic:"DIF1"  version:u8  codec:u8  raw_len:u64   compressed_body:[u8]
//! ```
//!
//! Codecs should prefer a pure-Rust implementation so the decoder compiles to
//! wasm; native-only codecs (e.g. `Zstd`) are allowed but unavailable in the
//! wasm decoder. The benchmark studies many more codecs over the `.difr` body.

use alloc::vec::Vec;

use crate::error::{DifError, Result};
use crate::format::{self, VERSION};
use crate::DifImage;

const MAGIC_DIF: [u8; 4] = *b"DIF1";

/// Compression algorithm used for a `.dif` body.
///
/// `Store`/`Deflate`/`Xz` are pure-Rust and decode in the default no_std build.
/// `Brotli` requires the `std` feature; `Zstd` requires `native` (C-linked
/// `zstd-safe`). Both are unavailable in a no_std build.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum CodecId {
    /// No compression — body stored verbatim.
    Store = 0,
    /// DEFLATE via `miniz_oxide`.
    Deflate = 1,
    /// Brotli (`brotli` crate). Requires the `std` feature. Default for production files.
    Brotli = 2,
    /// XZ — pure-Rust `lzma-rust2` (decode), `xz2`/liblzma encode under `native`.
    Xz = 3,
    /// Zstandard via `zstd-safe`. Requires the `native` feature.
    Zstd = 4,
}

impl CodecId {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(CodecId::Store),
            1 => Ok(CodecId::Deflate),
            2 => Ok(CodecId::Brotli),
            3 => Ok(CodecId::Xz),
            4 => Ok(CodecId::Zstd),
            _ => Err(DifError::BadCodec(v)),
        }
    }
}

fn compress(codec: CodecId, data: &[u8]) -> Result<Vec<u8>> {
    match codec {
        CodecId::Store => Ok(data.to_vec()),
        CodecId::Deflate => Ok(miniz_oxide::deflate::compress_to_vec(data, 6)),
        CodecId::Brotli => brotli_compress(data),
        CodecId::Xz => xz_compress(data),
        CodecId::Zstd => zstd_compress(data),
    }
}

fn decompress(codec: CodecId, data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    match codec {
        CodecId::Store => Ok(data.to_vec()),
        CodecId::Deflate => miniz_oxide::inflate::decompress_to_vec(data)
            .map_err(|_| DifError::CompressionFailed),
        CodecId::Brotli => brotli_decompress(data, raw_len),
        CodecId::Xz => xz_decompress(data, raw_len),
        CodecId::Zstd => zstd_decompress(data, raw_len),
    }
}

// --- Brotli: std-only (the `brotli` crate's streaming Reader/Writer need std) ---

#[cfg(feature = "std")]
fn brotli_compress(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write as _;
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 4096, 9, 22);
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
fn brotli_compress(_data: &[u8]) -> Result<Vec<u8>> {
    Err(DifError::Invalid("brotli codec requires the `std` feature"))
}

#[cfg(not(feature = "std"))]
fn brotli_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("brotli codec requires the `std` feature"))
}

// --- XZ: pure-Rust lzma-rust2 by default; faster liblzma (xz2) under `native` ---

#[cfg(not(feature = "native"))]
fn xz_compress(data: &[u8]) -> Result<Vec<u8>> {
    // lzma-rust2 is built without its `std` feature, so it exposes its own
    // no_std `Read`/`Write` traits and `Vec<u8>` writer.
    use lzma_rust2::{Write as _, XzOptions, XzWriter};
    let mut w = XzWriter::new(Vec::new(), XzOptions::with_preset(6))
        .map_err(|_| DifError::CompressionFailed)?;
    w.write_all(data).map_err(|_| DifError::CompressionFailed)?;
    w.finish().map_err(|_| DifError::CompressionFailed)
}

#[cfg(not(feature = "native"))]
fn xz_decompress(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    use lzma_rust2::{Read as _, XzReader};
    // Body length is known, so fill an exact buffer with `read_exact` — no
    // `read_to_end` (an alloc/std helper) needed.
    let mut out = alloc::vec![0u8; raw_len];
    XzReader::new(data, true)
        .read_exact(&mut out)
        .map_err(|_| DifError::CompressionFailed)?;
    Ok(out)
}

#[cfg(feature = "native")]
fn xz_compress(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write as _;
    let mut e = xz2::write::XzEncoder::new(Vec::new(), 6);
    e.write_all(data).map_err(|_| DifError::CompressionFailed)?;
    e.finish().map_err(|_| DifError::CompressionFailed)
}

#[cfg(feature = "native")]
fn xz_decompress(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::with_capacity(raw_len);
    xz2::read::XzDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|_| DifError::CompressionFailed)?;
    Ok(out)
}

// --- Zstd: only with the `native` feature (zstd-safe links C zstd) ---

#[cfg(feature = "native")]
fn zstd_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = vec![0u8; zstd_safe::compress_bound(data.len())];
    let n = zstd_safe::compress(out.as_mut_slice(), data, 19)
        .map_err(|_| DifError::CompressionFailed)?;
    out.truncate(n);
    Ok(out)
}

#[cfg(feature = "native")]
fn zstd_decompress(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; raw_len];
    let n = zstd_safe::decompress(out.as_mut_slice(), data)
        .map_err(|_| DifError::CompressionFailed)?;
    out.truncate(n);
    Ok(out)
}

#[cfg(not(feature = "native"))]
fn zstd_compress(_data: &[u8]) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zstd codec requires the `native` feature"))
}

#[cfg(not(feature = "native"))]
fn zstd_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zstd codec requires the `native` feature"))
}

/// Serialize and compress an image into a `.dif` container.
pub fn to_dif(img: &DifImage, codec: CodecId) -> Result<Vec<u8>> {
    img.validate()?;
    let mut body = Vec::new();
    format::write_body(img, &mut body);
    let compressed = compress(codec, &body)?;

    let mut out = Vec::with_capacity(compressed.len() + 14);
    out.extend_from_slice(&MAGIC_DIF);
    out.push(VERSION);
    out.push(codec as u8);
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

/// Parse and decompress a `.dif` container.
pub fn from_dif(bytes: &[u8]) -> Result<DifImage> {
    if bytes.len() < 14 {
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
    let raw_len = u64::from_le_bytes(bytes[6..14].try_into().unwrap()) as usize;
    let body = decompress(codec, &bytes[14..], raw_len)?;
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
                Theme { tag: ModeTag::Light, name: "light".into() },
                Theme { tag: ModeTag::Dark, name: "dark".into() },
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
        // `mut`/pushes are feature-gated; with no features only the base set exists.
        #[allow(unused_mut)]
        let mut codecs = vec![CodecId::Store, CodecId::Deflate, CodecId::Xz];
        #[cfg(feature = "std")]
        codecs.push(CodecId::Brotli);
        #[cfg(feature = "native")]
        codecs.push(CodecId::Zstd);
        for codec in codecs {
            let bytes = to_dif(&img, codec).unwrap();
            assert_eq!(&bytes[..4], b"DIF1");
            assert_eq!(bytes[5], codec as u8);
            let back = from_dif(&bytes).unwrap();
            assert_eq!(img, back, "codec {codec:?}");
        }
    }

    #[test]
    fn xz_cross_lib_interop() {
        // A .dif written here (lzma-rust2 by default, xz2 under `native`) must
        // round-trip through the decoder regardless of which lib produced it.
        let img = sample();
        let bytes = to_dif(&img, CodecId::Xz).unwrap();
        assert_eq!(from_dif(&bytes).unwrap(), img);
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
            themes: vec![Theme { tag: ModeTag::Light, name: "l".into() }],
            content: Content::Indexed {
                palettes: vec![pal],
                frames: vec![frame],
            },
            frame_delays: vec![0],
        };
        let store = to_dif(&img, CodecId::Store).unwrap().len();
        let brotli = to_dif(&img, CodecId::Brotli).unwrap().len();
        assert!(brotli < store, "brotli {brotli} should beat store {store}");
    }

    #[test]
    fn bad_codec_byte_rejected() {
        let mut bytes = to_dif(&sample(), CodecId::Store).unwrap();
        bytes[5] = 99;
        assert!(matches!(from_dif(&bytes), Err(DifError::BadCodec(99))));
    }
}
