//! DIF v3 container: the fixed 64-byte header (see [`crate::format`]) plus a
//! two-stage codec body.
//!
//! A codec byte packs a 4-bit **family** and a 4-bit **level index** into a
//! per-family level table (so e.g. `zstd-22`, which overflows 4 raw bits, is
//! reachable as a table index). Family 0 is `common-pick` — a benchmark-derived
//! table of recommended presets; `0/0` is `Store`.
//!
//! Encoding builds the *fully-decompressed* sections (themes, palettes, index
//! planes), compresses each palette/frame section independently (`codec_palette`
//! / `codec_frame`) into the **intermediate body**, then wraps the whole
//! intermediate body in the outer `codec`. With the outer codec set to `Store`
//! the header offsets index frames directly for random-access / low-memory decode.
//!
//! Decode is level-agnostic: every supported codec's stream is self-describing,
//! so only the family (→ method) and the known raw length are needed.

use alloc::vec::Vec;

use crate::error::{DifError, Result};
use crate::format::{self, align16, Header, HEADER_LEN};
use crate::{DifImage, Frame};

/// On-disk codec **family** (the high nibble of a codec byte).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum CodecId {
    /// Benchmark-derived preset table; `level 0` = Store (no compression).
    Common = 0,
    /// DEFLATE (`miniz_oxide` decode; `libdeflater` encode under `native`).
    Deflate = 1,
    /// Brotli (`brotli` crate). Requires the `std` feature.
    Brotli = 2,
    /// zxc (BSD-3) via the `zxc-compress` crate. Requires the `zxc` feature.
    Zxc = 3,
    /// Zstandard via `zstd-safe`. Requires a C-codec feature.
    Zstd = 4,
    /// LZ4 block via `lz4_flex` (pure-Rust, portable + wasm-decodable).
    Lz4 = 5,
    /// LZAV via the vendored `lzav-shim` C shim. Requires a C-codec feature.
    Lzav = 6,
}

/// Internal decode/encode method a codec byte resolves to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Method {
    Store,
    Deflate,
    Brotli,
    Zxc,
    Zstd,
    Lz4,
    Lzav,
}

// Per-family level tables. The level nibble indexes these; the value is the real
// (nominal) level recorded for provenance and used by encoders that honor it.
const DEFLATE_LEVELS: [i32; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
const BROTLI_LEVELS: [i32; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
// zxc levels 1..=6 (1 fastest, 6 densest).
const ZXC_LEVELS: [i32; 6] = [1, 2, 3, 4, 5, 6];
const ZSTD_LEVELS: [i32; 16] = [-7, -5, -3, -1, 1, 2, 3, 6, 8, 10, 12, 14, 16, 18, 20, 22];
// LZ4: positive = `lz4_flex` fast acceleration, negative = HC level (provenance
// only; `lz4_flex` exposes the fast block path, so the encoder ignores the value).
const LZ4_LEVELS: [i32; 13] = [99, 64, 32, 16, 8, 4, 2, 1, -1, -3, -6, -9, -12];
const LZAV_LEVELS: [i32; 2] = [1, 2];

/// A packed codec byte: `(family << 4) | level_index`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Codec(pub u8);

impl Codec {
    pub fn new(family: CodecId, level_index: u8) -> Self {
        Codec(((family as u8) << 4) | (level_index & 0xF))
    }

    /// `Store` — common-pick family, level 0.
    pub fn store() -> Self {
        Codec(0x00)
    }

    pub fn family(self) -> u8 {
        self.0 >> 4
    }
    pub fn level_index(self) -> usize {
        (self.0 & 0xF) as usize
    }

    fn resolve(self) -> Result<(Method, i32)> {
        let lvl = self.level_index();
        let pick = |t: &[i32]| t.get(lvl).copied().ok_or(DifError::BadCodec(self.0));
        match self.family() {
            0 => match lvl {
                0 => Ok((Method::Store, 0)),
                _ => Err(DifError::BadCodec(self.0)), // common-pick presets TBD
            },
            1 => Ok((Method::Deflate, pick(&DEFLATE_LEVELS)?)),
            2 => Ok((Method::Brotli, pick(&BROTLI_LEVELS)?)),
            3 => Ok((Method::Zxc, pick(&ZXC_LEVELS)?)),
            4 => Ok((Method::Zstd, pick(&ZSTD_LEVELS)?)),
            5 => Ok((Method::Lz4, pick(&LZ4_LEVELS)?)),
            6 => Ok((Method::Lzav, pick(&LZAV_LEVELS)?)),
            _ => Err(DifError::BadCodec(self.0)),
        }
    }

    /// Parse a study variant string (`"store"`, `"zstd-3"`, `"brotli-11"`,
    /// `"lz4-fast1"`, `"lzav-1"`, `"libdeflate-6"`, …) into a codec byte. Bare
    /// family names alias their study-chosen default level. Single source of truth
    /// for the per-family level semantics shared with the Python binding.
    pub fn parse(name: &str) -> Result<Codec> {
        fn idx(table: &[i32], v: i32) -> Option<u8> {
            table.iter().position(|&x| x == v).map(|p| p as u8)
        }
        let bad = || DifError::Invalid("unknown codec variant string");
        let (fam, real): (CodecId, i32) = match name {
            "store" => return Ok(Codec::store()),
            "deflate" | "libdeflate" | "deflate-6" | "libdeflate-6" => (CodecId::Deflate, 6),
            "brotli" | "brotli-5" => (CodecId::Brotli, 5),
            "brotli-11" => (CodecId::Brotli, 11),
            "zstd" | "zstd-3" => (CodecId::Zstd, 3),
            "zstd-10" => (CodecId::Zstd, 10),
            "zstd-22" => (CodecId::Zstd, 22),
            "lz4" | "lz4-fast1" => (CodecId::Lz4, 1),
            "lzav" | "lzav-1" => (CodecId::Lzav, 1),
            "zxc" | "zxc-3" => (CodecId::Zxc, 3),
            "zxc-1" => (CodecId::Zxc, 1),
            "zxc-2" => (CodecId::Zxc, 2),
            "zxc-4" => (CodecId::Zxc, 4),
            "zxc-5" => (CodecId::Zxc, 5),
            "zxc-6" => (CodecId::Zxc, 6),
            _ => return Err(bad()),
        };
        let table: &[i32] = match fam {
            CodecId::Deflate => &DEFLATE_LEVELS,
            CodecId::Brotli => &BROTLI_LEVELS,
            CodecId::Zxc => &ZXC_LEVELS,
            CodecId::Zstd => &ZSTD_LEVELS,
            CodecId::Lz4 => &LZ4_LEVELS,
            CodecId::Lzav => &LZAV_LEVELS,
            _ => return Err(bad()),
        };
        let li = idx(table, real).ok_or_else(bad)?;
        Ok(Codec::new(fam, li))
    }
}

// --- section (de)compression ---------------------------------------------

fn compress_section(codec: Codec, raw: &[u8], workers: u32) -> Result<Vec<u8>> {
    let (method, level) = codec.resolve()?;
    compress(method, level, raw, workers)
}

fn decompress_section(codec: Codec, data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    let (method, _) = codec.resolve()?;
    decompress(method, data, raw_len)
}

fn compress(method: Method, level: i32, data: &[u8], workers: u32) -> Result<Vec<u8>> {
    match method {
        Method::Store => Ok(data.to_vec()),
        Method::Deflate => deflate_compress(data, level),
        Method::Brotli => brotli_compress(data, level.clamp(0, 11) as u8, workers),
        Method::Zstd => zstd_compress(data, level, workers),
        Method::Lz4 => Ok(lz4_flex::block::compress(data)),
        Method::Lzav => lzav_compress(data),
        Method::Zxc => zxc_compress(data, level),
    }
}

fn decompress(method: Method, data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    match method {
        Method::Store => {
            if data.len() < raw_len {
                return Err(DifError::UnexpectedEof);
            }
            Ok(data[..raw_len].to_vec())
        }
        Method::Deflate => {
            miniz_oxide::inflate::decompress_to_vec(data).map_err(|_| DifError::CompressionFailed)
        }
        Method::Brotli => brotli_decompress(data, raw_len),
        Method::Zstd => zstd_decompress(data, raw_len),
        Method::Lz4 => {
            lz4_flex::block::decompress(data, raw_len).map_err(|_| DifError::CompressionFailed)
        }
        Method::Lzav => lzav_decompress(data, raw_len),
        Method::Zxc => zxc_decompress(data, raw_len),
    }
}

// --- Deflate: miniz_oxide encoder by default; libdeflate encoder under `native`.
//     Both emit a standard raw DEFLATE stream, decoded by miniz_oxide everywhere.

#[cfg(not(feature = "native"))]
fn deflate_compress(data: &[u8], level: i32) -> Result<Vec<u8>> {
    Ok(miniz_oxide::deflate::compress_to_vec(
        data,
        level.clamp(0, 10) as u8,
    ))
}

#[cfg(feature = "native")]
fn deflate_compress(data: &[u8], level: i32) -> Result<Vec<u8>> {
    use libdeflater::{CompressionLvl, Compressor};
    let lvl = CompressionLvl::new(level.clamp(1, 12)).map_err(|_| DifError::CompressionFailed)?;
    let mut c = Compressor::new(lvl);
    let mut out = alloc::vec![0u8; c.deflate_compress_bound(data.len())];
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

// --- zxc: BSD-3 C lib via the `zxc-compress` crate (feature `zxc`). The stream
//     is self-describing, so decode ignores the known raw length. ---

#[cfg(feature = "zxc")]
fn zxc_level(level: i32) -> zxc::Level {
    match level {
        1 => zxc::Level::Fastest,
        2 => zxc::Level::Fast,
        3 => zxc::Level::Default,
        4 => zxc::Level::Balanced,
        5 => zxc::Level::Compact,
        _ => zxc::Level::Density,
    }
}

#[cfg(feature = "zxc")]
fn zxc_compress(data: &[u8], level: i32) -> Result<Vec<u8>> {
    zxc::compress(data, zxc_level(level), None).map_err(|_| DifError::CompressionFailed)
}

#[cfg(feature = "zxc")]
fn zxc_decompress(data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    zxc::decompress(data).map_err(|_| DifError::CompressionFailed)
}

#[cfg(not(feature = "zxc"))]
fn zxc_compress(_data: &[u8], _level: i32) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zxc codec requires the `zxc` feature"))
}

#[cfg(not(feature = "zxc"))]
fn zxc_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zxc codec requires the `zxc` feature"))
}

// --- Brotli: std-only (the `brotli` crate's streaming Reader/Writer need std) ---

#[cfg(feature = "std")]
#[cfg_attr(not(feature = "brotli-mt"), allow(unused_variables))]
fn brotli_compress(data: &[u8], level: u8, workers: u32) -> Result<Vec<u8>> {
    #[cfg(feature = "brotli-mt")]
    if workers > 1 && level >= 10 {
        return brotli_compress_mt(data, level, workers);
    }
    use std::io::Write as _;
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 4096, level as u32, 22);
        w.write_all(data).map_err(|_| DifError::CompressionFailed)?;
    }
    Ok(out)
}

#[cfg(feature = "brotli-mt")]
fn brotli_compress_mt(data: &[u8], level: u8, workers: u32) -> Result<Vec<u8>> {
    use brotli::enc::multithreading::compress_multi;
    use brotli::enc::{
        Allocator, BrotliEncoderMaxCompressedSizeMulti, BrotliEncoderParams, Owned, SendAlloc,
        SliceWrapperMut, StandardAlloc, UnionHasher,
    };

    let params = BrotliEncoderParams {
        quality: level as i32,
        lgwin: 22,
        ..Default::default()
    };

    let mut ialloc = StandardAlloc::default();
    let mut input = <StandardAlloc as Allocator<u8>>::alloc_cell(&mut ialloc, data.len());
    input.slice_mut().copy_from_slice(data);
    let mut owned = Owned::new(input);

    let nthreads = (workers as usize).clamp(1, 16);
    let mut allocs = [
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
        SendAlloc::new(StandardAlloc::default(), UnionHasher::Uninit),
    ];
    let mut out = alloc::vec![0u8; BrotliEncoderMaxCompressedSizeMulti(data.len(), nthreads)];
    let n = compress_multi(&params, &mut owned, &mut out, &mut allocs[..nthreads])
        .map_err(|_| DifError::CompressionFailed)?;
    out.truncate(n);
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
fn brotli_compress(_data: &[u8], _level: u8, _workers: u32) -> Result<Vec<u8>> {
    Err(DifError::Invalid("brotli codec requires the `std` feature"))
}

#[cfg(not(feature = "std"))]
fn brotli_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("brotli codec requires the `std` feature"))
}

// --- Zstd: with the C-codec feature set (zstd-safe links C zstd) ---

#[cfg(feature = "c-codecs")]
#[cfg_attr(not(feature = "zstd-mt"), allow(unused_variables))]
fn zstd_compress(data: &[u8], level: i32, workers: u32) -> Result<Vec<u8>> {
    #[cfg(feature = "zstd-mt")]
    if workers > 0 {
        use zstd_safe::{CCtx, CParameter};
        let mut cctx = CCtx::create();
        cctx.set_parameter(CParameter::CompressionLevel(level))
            .map_err(|_| DifError::CompressionFailed)?;
        cctx.set_parameter(CParameter::NbWorkers(workers))
            .map_err(|_| DifError::CompressionFailed)?;
        let mut out = alloc::vec![0u8; zstd_safe::compress_bound(data.len())];
        let n = cctx
            .compress2(out.as_mut_slice(), data)
            .map_err(|_| DifError::CompressionFailed)?;
        out.truncate(n);
        return Ok(out);
    }
    let mut out = alloc::vec![0u8; zstd_safe::compress_bound(data.len())];
    let n = zstd_safe::compress(out.as_mut_slice(), data, level)
        .map_err(|_| DifError::CompressionFailed)?;
    out.truncate(n);
    Ok(out)
}

#[cfg(feature = "c-codecs")]
fn zstd_decompress(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    let mut out = alloc::vec![0u8; raw_len];
    let n =
        zstd_safe::decompress(out.as_mut_slice(), data).map_err(|_| DifError::CompressionFailed)?;
    out.truncate(n);
    Ok(out)
}

#[cfg(not(feature = "c-codecs"))]
fn zstd_compress(_data: &[u8], _level: i32, _workers: u32) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zstd codec requires a C-codec feature"))
}

#[cfg(not(feature = "c-codecs"))]
fn zstd_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zstd codec requires a C-codec feature"))
}

// --- container assembly ---------------------------------------------------

fn encode(
    img: &DifImage,
    is_raw: bool,
    outer: Codec,
    palette: Codec,
    frame: Codec,
    workers: u32,
) -> Result<Vec<u8>> {
    img.validate()?;
    let depth = img.color_depth;
    let iw = img.index_width;
    let t = img.themes.len();
    let cc = img.index_count();

    // Build the intermediate body: themes | pad16 | palette blob | pad16 | frames.
    let mut mid: Vec<u8> = Vec::new();
    format::themes_bytes(&img.themes, &mut mid);
    pad_to_16(&mut mid); // palette begins here, at align16(4 * t)

    let mut raw_pal: Vec<u8> = Vec::new();
    format::palettes_bytes(&img.palettes, depth, &mut raw_pal);
    let pal_blob = compress_section(palette, &raw_pal, workers)?;
    let palette_size = pal_blob.len();
    mid.extend_from_slice(&pal_blob);
    pad_to_16(&mut mid);
    let first_frame_start = mid.len();

    // Compress each frame, then pick a uniform 16-aligned stride.
    let mut blobs: Vec<Vec<u8>> = Vec::with_capacity(img.frames.len());
    for f in &img.frames {
        let mut raw_bm: Vec<u8> = Vec::new();
        format::frame_bitmap_bytes(&f.indices, iw, &mut raw_bm);
        blobs.push(compress_section(frame, &raw_bm, workers)?);
    }
    let alignment = blobs
        .iter()
        .map(|b| align16(12 + b.len()))
        .max()
        .unwrap_or(16)
        .max(16);

    for (f, blob) in img.frames.iter().zip(&blobs) {
        let record_start = mid.len();
        let size = (12 + blob.len()) as u64; // size field + delay + content
        mid.extend_from_slice(&size.to_le_bytes());
        mid.extend_from_slice(&f.delay_us.to_le_bytes());
        mid.extend_from_slice(blob);
        mid.resize(record_start + alignment, 0);
    }

    let body = compress_section(outer, &mid, workers)?;

    let header = Header {
        is_raw,
        codec: outer.0,
        flags: iw.to_bits() | (depth.to_bits() << 2),
        codec_palette: palette.0,
        codec_frame: frame.0,
        theme_count: t,
        frame_count: img.frames.len(),
        replay_count: img.replay_count,
        width: img.width,
        height: img.height,
        first_frame_offset: (HEADER_LEN + first_frame_start) as u128,
        frame_alignment: alignment as u64,
        index_count: cc as u64,
        palette_size: palette_size as u64,
    };
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    header.write(&mut out);
    out.extend_from_slice(&body);
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<DifImage> {
    let h = Header::read(bytes)?;
    let depth = h.color_depth()?;
    let iw = h.index_width()?;
    let t = h.theme_count;
    if t == 0 || t > 256 {
        return Err(DifError::BadThemeCount(t));
    }
    let cc = h.index_count as usize;
    let px = h.width as usize * h.height as usize;
    let body = &bytes[HEADER_LEN..];

    let first_frame_internal = (h.first_frame_offset - HEADER_LEN as u128) as usize;
    let alignment = h.frame_alignment as usize;
    if alignment == 0 || !alignment.is_multiple_of(16) {
        return Err(DifError::Unaligned(alignment as u64));
    }
    let mid_len = first_frame_internal
        .checked_add(
            h.frame_count
                .checked_mul(alignment)
                .ok_or(DifError::UnexpectedEof)?,
        )
        .ok_or(DifError::UnexpectedEof)?;

    let mid = decompress_section(Codec(h.codec), body, mid_len)?;
    if mid.len() < mid_len {
        return Err(DifError::UnexpectedEof);
    }

    let themes = format::read_themes(&mid, t)?;

    let palette_start = align16(t * 4);
    let palette_size = h.palette_size as usize;
    let pal_end = palette_start
        .checked_add(palette_size)
        .ok_or(DifError::UnexpectedEof)?;
    if pal_end > mid.len() || first_frame_internal < palette_start {
        return Err(DifError::Invalid("palette section out of bounds"));
    }
    let pal_blob = &mid[palette_start..pal_end];
    let raw_pal_len = t * cc * depth.color_bytes();
    let raw_pal = decompress_section(Codec(h.codec_palette), pal_blob, raw_pal_len)?;
    let palettes = format::read_palettes(&raw_pal, t, cc, depth)?;

    let mut frames = Vec::with_capacity(h.frame_count);
    let bm_len = px * iw.bytes();
    for j in 0..h.frame_count {
        let fstart = first_frame_internal + j * alignment;
        if fstart + 12 > mid.len() {
            return Err(DifError::UnexpectedEof);
        }
        let size = u64::from_le_bytes(mid[fstart..fstart + 8].try_into().unwrap()) as usize;
        let delay = u32::from_le_bytes(mid[fstart + 8..fstart + 12].try_into().unwrap());
        if size < 12 || fstart + size > mid.len() {
            return Err(DifError::Invalid("bad frame record size"));
        }
        let blob = &mid[fstart + 12..fstart + size];
        let raw_bm = decompress_section(Codec(h.codec_frame), blob, bm_len)?;
        let indices = format::read_frame_bitmap(&raw_bm, px, iw)?;
        frames.push(Frame {
            delay_us: delay,
            indices,
        });
    }

    let img = DifImage {
        width: h.width,
        height: h.height,
        color_depth: depth,
        index_width: iw,
        themes,
        palettes,
        frames,
        replay_count: h.replay_count,
    };
    img.validate()?;
    Ok(img)
}

fn pad_to_16(buf: &mut Vec<u8>) {
    let n = align16(buf.len());
    buf.resize(n, 0);
}

/// Serialize and compress an image into a `.dif` container (single-thread).
pub fn to_dif(img: &DifImage, outer: Codec, palette: Codec, frame: Codec) -> Result<Vec<u8>> {
    to_dif_workers(img, outer, palette, frame, 0)
}

/// Like [`to_dif`], but `workers` > 0 runs the multithreaded zstd/brotli encoders
/// (other codecs ignore it). `workers` is encode-only — not stored — and the bytes
/// stay a standard container decoded by [`from_dif`].
pub fn to_dif_workers(
    img: &DifImage,
    outer: Codec,
    palette: Codec,
    frame: Codec,
    workers: u32,
) -> Result<Vec<u8>> {
    encode(img, false, outer, palette, frame, workers)
}

/// Serialize to a raw, uncompressed `.difr` container (all sections `Store`).
pub fn to_difr(img: &DifImage) -> Result<Vec<u8>> {
    encode(img, true, Codec::store(), Codec::store(), Codec::store(), 0)
}

/// Parse and decompress a `.dif` container.
pub fn from_dif(bytes: &[u8]) -> Result<DifImage> {
    let h = Header::read(bytes)?;
    if h.is_raw {
        return Err(DifError::BadMagic(format::MAGIC_DIFR));
    }
    decode(bytes)
}

/// Parse a raw `.difr` container.
pub fn from_difr(bytes: &[u8]) -> Result<DifImage> {
    let h = Header::read(bytes)?;
    if !h.is_raw {
        return Err(DifError::BadMagic(format::MAGIC_DIF));
    }
    decode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{abilities, ColorDepth, IndexWidth, Rgba, Theme, ThemeTag};
    use alloc::vec;

    fn sample(depth: ColorDepth, iw: IndexWidth) -> DifImage {
        let maxc = if depth == ColorDepth::Rgba8 {
            255
        } else {
            60000
        };
        let light = vec![Rgba::new(maxc, maxc, maxc, maxc), Rgba::new(0, 0, 0, maxc)];
        let dark = vec![Rgba::new(0, 0, 0, maxc), Rgba::new(maxc, maxc, maxc, maxc)];
        DifImage {
            width: 4,
            height: 4,
            color_depth: depth,
            index_width: iw,
            themes: vec![
                Theme {
                    abilities: abilities::LIGHT,
                    base_color: [255, 255, 255],
                },
                Theme {
                    abilities: abilities::DARK,
                    base_color: [0, 0, 0],
                },
            ],
            palettes: vec![light, dark],
            frames: vec![Frame {
                delay_us: 0,
                indices: vec![0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 1],
            }],
            replay_count: 1,
        }
    }

    #[test]
    fn difr_roundtrip_all_combos() {
        for depth in [ColorDepth::Rgba8, ColorDepth::Rgba16] {
            for iw in [IndexWidth::Eight, IndexWidth::Sixteen] {
                let img = sample(depth, iw);
                let bytes = to_difr(&img).unwrap();
                assert_eq!(&bytes[..5], b"DIFR3");
                let back = from_difr(&bytes).unwrap();
                assert_eq!(img, back, "depth {depth:?} width {iw:?}");
            }
        }
    }

    #[test]
    fn dif_roundtrip_codec_matrix() {
        let img = sample(ColorDepth::Rgba8, IndexWidth::Eight);
        #[allow(unused_mut)]
        let mut codecs = vec!["store", "deflate", "lz4"];
        #[cfg(feature = "std")]
        codecs.push("brotli-5");
        #[cfg(feature = "native")]
        {
            codecs.push("zstd-3");
            codecs.push("lzav-1");
            codecs.push("zxc-3");
        }
        for name in codecs {
            let c = Codec::parse(name).unwrap();
            // Try it as outer, as palette section, and as frame section.
            for (o, p, f) in [(c, Codec::store(), Codec::store()), (Codec::store(), c, c)] {
                let bytes = to_dif_workers(&img, o, p, f, 0).unwrap();
                assert_eq!(&bytes[..4], b"DIF3");
                let back = from_dif(&bytes).unwrap();
                assert_eq!(img, back, "codec {name} o={o:?} p={p:?} f={f:?}");
            }
        }
    }

    #[test]
    fn multi_frame_random_access_offsets() {
        let mut img = sample(ColorDepth::Rgba8, IndexWidth::Eight);
        img.frames = vec![
            Frame {
                delay_us: 100,
                indices: vec![0u64; 16],
            },
            Frame {
                delay_us: 200,
                indices: vec![1u64; 16],
            },
            Frame {
                delay_us: 300,
                indices: vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1],
            },
        ];
        img.replay_count = 0;
        let bytes = to_difr(&img).unwrap();
        let back = from_difr(&bytes).unwrap();
        assert_eq!(img, back);
        assert_eq!(back.frames[1].delay_us, 200);
        assert_eq!(back.replay_count, 0);
    }

    #[test]
    fn render_picks_dark_theme() {
        let img = sample(ColorDepth::Rgba8, IndexWidth::Eight);
        // dark palette: idx0=black, idx1=white. frame[0..2] = [0,1].
        let rgba = img.render_rgba8(ThemeTag::Dark, [0, 0, 0], 0).unwrap();
        assert_eq!(&rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn parse_known_variants() {
        assert_eq!(Codec::parse("store").unwrap(), Codec(0x00));
        assert_eq!(
            Codec::parse("zstd-3").unwrap(),
            Codec::new(CodecId::Zstd, 6)
        );
        assert_eq!(
            Codec::parse("zstd-22").unwrap(),
            Codec::new(CodecId::Zstd, 15)
        );
        assert_eq!(
            Codec::parse("brotli-11").unwrap(),
            Codec::new(CodecId::Brotli, 11)
        );
        assert_eq!(
            Codec::parse("lz4-fast1").unwrap(),
            Codec::new(CodecId::Lz4, 7)
        );
        assert!(Codec::parse("nope").is_err());
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = to_dif(
            &sample(ColorDepth::Rgba8, IndexWidth::Eight),
            Codec::store(),
            Codec::store(),
            Codec::store(),
        )
        .unwrap();
        bytes[0] = b'X';
        assert!(matches!(from_dif(&bytes), Err(DifError::BadMagic(_))));
    }

    #[test]
    fn reject_32bit_index_width() {
        let mut bytes = to_difr(&sample(ColorDepth::Rgba8, IndexWidth::Eight)).unwrap();
        bytes[9] = (bytes[9] & !0b11) | 0b10; // index width -> 32-bit
        assert!(matches!(
            from_difr(&bytes),
            Err(DifError::BadIndexWidth(32))
        ));
    }
}
