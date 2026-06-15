//! DIF v3 container: the fixed 64-byte header (see [`crate::format`]) plus a
//! two-stage codec body.
//!
//! A codec byte packs a 4-bit **family** and a 4-bit **level index** into a
//! per-family level table (so e.g. `zstd-22`, which overflows 4 raw bits, is
//! reachable as a table index). Family 0 is `common-pick` --- a benchmark-derived
//! table of recommended presets; `0/0` is `Store`.
//!
//! Encoding builds the *fully-decompressed* sections (themes, palettes, index
//! planes), compresses each palette/frame section independently (`codec_palette`
//! / `codec_frame`) into the **intermediate body**, then wraps the whole
//! intermediate body in the outer `codec`. With the outer codec set to `Store`
//! the header offsets index frames directly for random-access / low-memory decode.
//!
//! Decode is level-agnostic: every supported codec's stream is self-describing,
//! so only the family (-> method) and the known raw length are needed.

use alloc::vec::Vec;

use crate::error::{DifError, Result};
use crate::format::{self, HEADER_LEN, Header, align16};
use crate::{DifImage, Frame, IndexWidth};

/// Default in-frame split job size (`J`): the controlled per-job byte target the
/// scheduler hands zstd, overriding its ~1 MB internal floor so multi-MB frames
/// stay few, big jobs (keeps cross-job matches, bounds the ratio tax). Only used
/// when `frame_count < workers` forces an in-frame split.
const DEFAULT_FRAME_JOB_SIZE: usize = 4 << 20; // 4 MiB

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
    /// libbsc BWT (Apache-2.0) via the vendored C++ shim. Requires the `bsc`
    /// feature; native-only (no wasm decode).
    Bsc = 3,
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
    Bsc,
    Zstd,
    Lz4,
    Lzav,
}

// Per-family level tables. The level nibble indexes these; the value is the real
// (nominal) level recorded for provenance and used by encoders that honor it.
const DEFLATE_LEVELS: [i32; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
const BROTLI_LEVELS: [i32; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
// libbsc levels: QLFC coder 1=fast, 2=static (default), 3=adaptive (densest),
// all over a BWT block. Provenance + the value the shim maps to a coder.
const BSC_LEVELS: [i32; 3] = [1, 2, 3];
const ZSTD_LEVELS: [i32; 16] = [-7, -5, -3, -1, 1, 2, 3, 6, 8, 10, 12, 14, 16, 18, 20, 22];
// LZ4: sign aligned with Zstandard (negative = fast) --- negative = `lz4_flex` fast
// acceleration (|level| = accel), positive = HC level. Provenance only; `lz4_flex`
// exposes the fast block path, so the encoder ignores the value.
const LZ4_LEVELS: [i32; 16] = [
    -512, -256, -128, -64, -32, -16, -8, -4, -2, -1, 2, 4, 6, 9, 10, 12,
];
const LZAV_LEVELS: [i32; 2] = [1, 2];

/// A packed codec byte: `(family << 4) | level_index`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Codec(pub u8);

impl Codec {
    pub fn new(family: CodecId, level_index: u8) -> Self {
        Codec(((family as u8) << 4) | (level_index & 0xF))
    }

    /// `Store` --- common-pick family, level 0.
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
            3 => Ok((Method::Bsc, pick(&BSC_LEVELS)?)),
            4 => Ok((Method::Zstd, pick(&ZSTD_LEVELS)?)),
            5 => Ok((Method::Lz4, pick(&LZ4_LEVELS)?)),
            6 => Ok((Method::Lzav, pick(&LZAV_LEVELS)?)),
            _ => Err(DifError::BadCodec(self.0)),
        }
    }

    /// Parse a study variant string (`"store"`, `"zstd-3"`, `"brotli-11"`,
    /// `"lz4-fast1"`, `"lzav-1"`, `"libdeflate-6"`, ...) into a codec byte. Bare
    /// family names alias their study-chosen default level. Single source of truth
    /// for the per-family level semantics shared with the Python binding.
    pub fn parse(name: &str) -> Result<Codec> {
        fn idx(table: &[i32], v: i32) -> Option<u8> {
            table.iter().position(|&x| x == v).map(|p| p as u8)
        }
        // LZ4 spells its level `fast<n>` (negative accel) / `hc<n>` (positive HC
        // level); every other family takes the bare nominal level integer.
        fn parse_level(fam: CodecId, s: &str) -> Option<i32> {
            if fam == CodecId::Lz4 {
                if let Some(r) = s.strip_prefix("fast") {
                    return r.parse::<i32>().ok().map(|v| -v);
                }
                if let Some(r) = s.strip_prefix("hc") {
                    return r.parse().ok();
                }
            }
            s.parse().ok()
        }
        let bad = || DifError::Invalid("unknown codec variant string");
        if name == "store" {
            return Ok(Codec::store());
        }
        // `<family>` aliases its study-default level; `<family>-<level>` selects any
        // level present in the family's table (see the per-family LEVELS arrays).
        let (fam_str, lvl_str) = match name.split_once('-') {
            Some((f, l)) => (f, Some(l)),
            None => (name, None),
        };
        let (fam, table, default): (CodecId, &[i32], i32) = match fam_str {
            "deflate" | "libdeflate" => (CodecId::Deflate, &DEFLATE_LEVELS, 6),
            "brotli" => (CodecId::Brotli, &BROTLI_LEVELS, 5),
            "bsc" => (CodecId::Bsc, &BSC_LEVELS, 2),
            "zstd" => (CodecId::Zstd, &ZSTD_LEVELS, 3),
            "lz4" => (CodecId::Lz4, &LZ4_LEVELS, -1),
            "lzav" => (CodecId::Lzav, &LZAV_LEVELS, 1),
            _ => return Err(bad()),
        };
        let real = match lvl_str {
            None => default,
            Some(l) => parse_level(fam, l).ok_or_else(bad)?,
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
        Method::Zstd => zstd_compress(data, level, workers, None),
        Method::Lz4 => Ok(lz4_flex::block::compress(data)),
        Method::Lzav => lzav_compress(data, level),
        Method::Bsc => bsc_compress(data, level),
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
        Method::Bsc => bsc_decompress(data, raw_len),
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

// --- Lzav: C shim (native or zig-cross wasm). `lzav-1` = default level,
//     `lzav-2` = high-ratio (`lzav_compress_hi`); decode is format-tagged. ---

#[cfg(feature = "c-codecs")]
fn lzav_compress(data: &[u8], level: i32) -> Result<Vec<u8>> {
    let out = if level >= 2 {
        lzav_shim::compress_hi(data)
    } else {
        lzav_shim::compress(data)
    };
    out.ok_or(DifError::CompressionFailed)
}

#[cfg(feature = "c-codecs")]
fn lzav_decompress(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    lzav_shim::decompress(data, raw_len).ok_or(DifError::CompressionFailed)
}

#[cfg(not(feature = "c-codecs"))]
fn lzav_compress(_data: &[u8], _level: i32) -> Result<Vec<u8>> {
    Err(DifError::Invalid("lzav codec requires a C-codec feature"))
}

#[cfg(not(feature = "c-codecs"))]
fn lzav_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("lzav codec requires a C-codec feature"))
}

// --- libbsc: BWT block compressor via the vendored C++ shim (feature `bsc`).
//     The shim tags every blob and decode is bounded by the known raw length. ---

#[cfg(feature = "bsc")]
fn bsc_compress(data: &[u8], level: i32) -> Result<Vec<u8>> {
    libbsc_shim::compress(data, level).ok_or(DifError::CompressionFailed)
}

#[cfg(feature = "bsc")]
fn bsc_decompress(data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    libbsc_shim::decompress(data, raw_len).ok_or(DifError::CompressionFailed)
}

#[cfg(not(feature = "bsc"))]
fn bsc_compress(_data: &[u8], _level: i32) -> Result<Vec<u8>> {
    Err(DifError::Invalid("bsc codec requires the `bsc` feature"))
}

#[cfg(not(feature = "bsc"))]
fn bsc_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("bsc codec requires the `bsc` feature"))
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
fn zstd_compress(data: &[u8], level: i32, workers: u32, job_size: Option<u32>) -> Result<Vec<u8>> {
    #[cfg(feature = "zstd-mt")]
    if workers > 0 {
        use zstd_safe::{CCtx, CParameter};
        let mut cctx = CCtx::create();
        cctx.set_parameter(CParameter::CompressionLevel(level))
            .map_err(|_| DifError::CompressionFailed)?;
        cctx.set_parameter(CParameter::NbWorkers(workers))
            .map_err(|_| DifError::CompressionFailed)?;
        // `J`: the scheduler-chosen per-job size; overrides zstd's ~1 MB floor so
        // the in-frame split is the controlled, ratio-bounded one.
        if let Some(js) = job_size
            && js > 0
        {
            cctx.set_parameter(CParameter::JobSize(js))
                .map_err(|_| DifError::CompressionFailed)?;
        }
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
fn zstd_compress(
    _data: &[u8],
    _level: i32,
    _workers: u32,
    _job_size: Option<u32>,
) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zstd codec requires a C-codec feature"))
}

#[cfg(not(feature = "c-codecs"))]
fn zstd_decompress(_data: &[u8], _raw_len: usize) -> Result<Vec<u8>> {
    Err(DifError::Invalid("zstd codec requires a C-codec feature"))
}

// --- frame parallelism ----------------------------------------------------

/// Run `f` over `0..n`, returning results indexed by `i`. With `threads >= 2`
/// and the `std` feature, a bounded pool of `min(threads, n)` scoped workers
/// pulls indices off a shared atomic counter (work-stealing-lite); output order
/// is by index, so the bytes are identical to the serial path regardless of how
/// many threads ran. Without `std`, or with `threads < 2`, it is a plain loop.
#[cfg(feature = "std")]
fn parallel_map<T, F>(n: usize, threads: usize, f: F) -> Result<Vec<T>>
where
    T: Send,
    F: Fn(usize) -> Result<T> + Sync,
{
    if threads < 2 || n < 2 {
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(f(i)?);
        }
        return Ok(out);
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    let counter = AtomicUsize::new(0);
    let counter = &counter;
    let f = &f;
    let nthreads = threads.min(n);
    let partials: Vec<Result<Vec<(usize, T)>>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..nthreads)
            .map(|_| {
                s.spawn(move || {
                    let mut local: Vec<(usize, T)> = Vec::new();
                    loop {
                        let i = counter.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        local.push((i, f(i)?));
                    }
                    Ok(local)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or(Err(DifError::CompressionFailed)))
            .collect()
    });
    let mut slots: Vec<Option<T>> = Vec::with_capacity(n);
    slots.resize_with(n, || None);
    for part in partials {
        for (i, v) in part? {
            slots[i] = Some(v);
        }
    }
    let mut out = Vec::with_capacity(n);
    for slot in slots {
        out.push(slot.ok_or(DifError::CompressionFailed)?);
    }
    Ok(out)
}

#[cfg(not(feature = "std"))]
fn parallel_map<T, F>(n: usize, _threads: usize, f: F) -> Result<Vec<T>>
where
    F: Fn(usize) -> Result<T>,
{
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(f(i)?);
    }
    Ok(out)
}

/// Compress one frame's index-plane bytes. `workers` is the in-frame split factor
/// `k`: `0`/`1` means a single serial job (pure inter-frame), `> 1` asks the codec
/// for a `k`-way single-blob native-MT split. Only zstd (`JobSize = J`) and brotli
/// (`level >= 10`, parallel meta-blocks) honor `k > 1`; every other family ignores
/// it and stays one job.
fn compress_frame(codec: Codec, raw: &[u8], workers: u32, job_size: u32) -> Result<Vec<u8>> {
    let (method, level) = codec.resolve()?;
    match method {
        Method::Zstd => zstd_compress(raw, level, workers, Some(job_size)),
        _ => compress(method, level, raw, workers),
    }
}

/// Compress every frame's index plane, returning blobs indexed by frame.
///
/// `frame_count >= workers`: pure inter-frame --- `min(workers, n)` threads, each
/// frame compressed serially (`k = 1`), zero ratio loss. `frame_count < workers`:
/// too few frames to fill the pool, so each frame is split `k`-ways
/// (`k = min(ceil(T/n), floor(S/J))`, capped by family eligibility inside
/// [`compress_frame`]), one thread per frame, so live threads ~ `n * k ~ T`.
fn compress_frames(
    frames: &[Frame],
    iw: IndexWidth,
    codec: Codec,
    workers: u32,
    job_size: usize,
) -> Result<Vec<Vec<u8>>> {
    let n = frames.len();
    let t = (workers.max(1) as usize).max(1);
    let (per_frame_k, pool_threads) = if n == 0 || n >= t {
        (0u32, t) // pure inter-frame; each codec runs single-threaded
    } else {
        // Few frames: split each across the spare threads.
        let s = frames.iter().map(|f| f.indices.len()).max().unwrap_or(0) * iw.bytes();
        let by_threads = t.div_ceil(n); // ceil(T / n)
        let by_size = (s / job_size.max(1)).max(1); // floor(S / J), >= 1
        let k = by_threads.min(by_size).max(1) as u32;
        (k, n)
    };
    let js = job_size.min(u32::MAX as usize) as u32;
    parallel_map(n, pool_threads, |j| {
        let mut raw_bm: Vec<u8> = Vec::new();
        format::frame_bitmap_bytes(&frames[j].indices, iw, &mut raw_bm);
        compress_frame(codec, &raw_bm, per_frame_k, js)
    })
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

    // Compress each frame (inter-frame parallel; in-frame split only when there
    // are too few frames to fill the pool), then pick a uniform 16-aligned stride.
    let blobs = compress_frames(&img.frames, iw, frame, workers, DEFAULT_FRAME_JOB_SIZE)?;
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

fn decode(bytes: &[u8], workers: u32) -> Result<DifImage> {
    let h = Header::read(bytes)?;
    let depth = h.color_depth()?;
    let iw = h.index_width()?;
    // The flags can name 32-/64-bit widths, but this build only handles 8-/16-bit.
    if !iw.supported() {
        return Err(DifError::BadIndexWidth((iw.bytes() * 8) as u8));
    }
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

    // Frames are independent streams at known offsets, so decode them in parallel
    // over the (already materialized, immutably shared) intermediate body.
    let bm_len = px * iw.bytes();
    let mid = &mid;
    let frames = parallel_map(h.frame_count, workers.max(1) as usize, |j| {
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
        Ok(Frame {
            delay_us: delay,
            indices,
        })
    })?;

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
/// (other codecs ignore it). `workers` is encode-only --- not stored --- and the bytes
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

/// Parse and decompress a `.dif` container (single-thread).
pub fn from_dif(bytes: &[u8]) -> Result<DifImage> {
    from_dif_workers(bytes, 1)
}

/// Like [`from_dif`], but `workers` > 1 decodes frames in parallel (opt-in; the
/// default stays serial). The result is identical regardless of worker count.
pub fn from_dif_workers(bytes: &[u8], workers: u32) -> Result<DifImage> {
    let h = Header::read(bytes)?;
    if h.is_raw {
        return Err(DifError::BadMagic(format::MAGIC_DIFR));
    }
    decode(bytes, workers)
}

/// Parse a raw `.difr` container.
pub fn from_difr(bytes: &[u8]) -> Result<DifImage> {
    let h = Header::read(bytes)?;
    if !h.is_raw {
        return Err(DifError::BadMagic(format::MAGIC_DIF));
    }
    decode(bytes, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColorDepth, IndexWidth, Rgba, Theme, ThemeTag, abilities};
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

    // A codec whose backend is not compiled into this feature set must surface a
    // clean error from both halves of its stub, never panic. Each block only
    // compiles when the corresponding stub is the one in the build.
    #[test]
    fn unavailable_codec_backends_error() {
        #[allow(unused_variables)]
        let data = b"some bytes that will not actually be compressed";
        #[cfg(not(feature = "std"))]
        {
            assert!(compress(Method::Brotli, 5, data, 0).is_err());
            assert!(decompress(Method::Brotli, data, data.len()).is_err());
        }
        #[cfg(not(feature = "c-codecs"))]
        {
            assert!(compress(Method::Zstd, 3, data, 0).is_err());
            assert!(decompress(Method::Zstd, data, data.len()).is_err());
            assert!(compress(Method::Lzav, 1, data, 0).is_err());
            assert!(decompress(Method::Lzav, data, data.len()).is_err());
        }
        #[cfg(not(feature = "bsc"))]
        {
            assert!(compress(Method::Bsc, 2, data, 0).is_err());
            assert!(decompress(Method::Bsc, data, data.len()).is_err());
        }
    }

    // frame_count < workers takes the in-frame-split planning branch (per-frame
    // `k`), which the native-only split tests cover with zstd/brotli. Repeat it
    // with a portable codec so every feature set exercises that branch too.
    #[test]
    fn fewer_frames_than_workers_plans_split_branch() {
        let mut img = sample(ColorDepth::Rgba8, IndexWidth::Bit8);
        img.frames = vec![
            Frame {
                delay_us: 0,
                indices: vec![0u64; 16],
            },
            Frame {
                delay_us: 0,
                indices: vec![1u64; 16],
            },
        ];
        let bytes =
            to_dif_workers(&img, Codec::store(), Codec::store(), Codec::store(), 8).unwrap();
        assert_eq!(from_dif(&bytes).unwrap(), img);
    }

    #[test]
    fn difr_roundtrip_all_combos() {
        for depth in [ColorDepth::Rgba8, ColorDepth::Rgba16] {
            for iw in [IndexWidth::Bit8, IndexWidth::Bit16] {
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
        let img = sample(ColorDepth::Rgba8, IndexWidth::Bit8);
        #[allow(unused_mut)]
        let mut codecs = vec!["store", "deflate", "lz4"];
        #[cfg(feature = "std")]
        codecs.push("brotli-5");
        #[cfg(feature = "native")]
        {
            codecs.push("zstd-3");
            codecs.push("lzav-1");
            codecs.push("lzav-2");
            codecs.push("bsc-2");
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
        let mut img = sample(ColorDepth::Rgba8, IndexWidth::Bit8);
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

    // A square `n`-frame image; each frame is a 0/1 index plane (palette has 2
    // colors) so it stays valid at any side length.
    #[cfg(feature = "std")]
    fn multi(n: usize, side: u32) -> DifImage {
        let mut img = sample(ColorDepth::Rgba8, IndexWidth::Bit8);
        img.width = side;
        img.height = side;
        let px = (side * side) as usize;
        img.frames = (0..n)
            .map(|f| Frame {
                delay_us: (f as u32) * 10,
                indices: (0..px).map(|i| ((i + f) % 2) as u64).collect(),
            })
            .collect();
        img
    }

    // frame_count >= workers => k=1 pure inter-frame: parallel must be
    // byte-identical to serial on both encode and decode.
    #[test]
    #[cfg(feature = "std")]
    fn parallel_matches_serial_encode_and_decode() {
        let img = multi(8, 4);
        let frame_codec = if cfg!(feature = "native") {
            Codec::parse("zstd-3").unwrap()
        } else {
            Codec::store()
        };
        let serial = to_dif_workers(&img, Codec::store(), Codec::store(), frame_codec, 0).unwrap();
        let parallel =
            to_dif_workers(&img, Codec::store(), Codec::store(), frame_codec, 8).unwrap();
        assert_eq!(serial, parallel, "k=1 inter-frame must equal serial bytes");
        let a = from_dif(&serial).unwrap();
        let b = from_dif_workers(&serial, 8).unwrap();
        assert_eq!(a, b, "parallel decode must equal serial decode");
        assert_eq!(img, b);
    }

    // frame_count < workers + small J forces k>1; the split must still yield one
    // decodable blob per frame (zstd JobSize path).
    #[test]
    #[cfg(feature = "native")]
    fn in_frame_split_zstd_blob_roundtrips() {
        let img = multi(2, 32);
        let iw = img.index_width;
        let zstd = Codec::parse("zstd-3").unwrap();
        let blobs = compress_frames(&img.frames, iw, zstd, 8, 256).unwrap();
        assert_eq!(blobs.len(), 2);
        let px = (img.width * img.height) as usize;
        for (f, blob) in img.frames.iter().zip(&blobs) {
            let raw = decompress_section(zstd, blob, px * iw.bytes()).unwrap();
            let idx = format::read_frame_bitmap(&raw, px, iw).unwrap();
            assert_eq!(&idx, &f.indices, "split zstd frame must roundtrip");
        }
    }

    // Brotli at level >= 10 is the second single-blob-MT family: split via
    // compress_multi, one decodable stream per frame.
    #[test]
    #[cfg(feature = "native")]
    fn in_frame_split_brotli_blob_roundtrips() {
        let img = multi(2, 32);
        let iw = img.index_width;
        let br = Codec::parse("brotli-11").unwrap();
        let blobs = compress_frames(&img.frames, iw, br, 8, 256).unwrap();
        let px = (img.width * img.height) as usize;
        for (f, blob) in img.frames.iter().zip(&blobs) {
            let raw = decompress_section(br, blob, px * iw.bytes()).unwrap();
            let idx = format::read_frame_bitmap(&raw, px, iw).unwrap();
            assert_eq!(&idx, &f.indices, "split brotli frame must roundtrip");
        }
    }

    // Non-MT families (and brotli < 10) ignore k>1 and stay one job; under thread
    // pressure (frame_count < workers) they must still roundtrip end to end.
    #[test]
    #[cfg(feature = "native")]
    fn no_split_codecs_roundtrip_under_thread_pressure() {
        let img = multi(2, 8);
        for name in ["lz4", "lzav-1", "deflate", "bsc-2", "brotli-5"] {
            let c = Codec::parse(name).unwrap();
            let bytes = to_dif_workers(&img, Codec::store(), Codec::store(), c, 8).unwrap();
            let back = from_dif_workers(&bytes, 8).unwrap();
            assert_eq!(img, back, "codec {name} under thread pressure");
        }
    }

    // A corrupt frame-record size must be rejected on both the serial and the
    // parallel decode path (and the error must propagate out of the pool).
    #[test]
    #[cfg(feature = "std")]
    fn decode_rejects_corrupt_frame_record() {
        let img = multi(3, 4);
        // Store outer keeps the file body == intermediate body, so the record's
        // size field is directly pokeable in the output bytes.
        let mut bytes =
            to_dif_workers(&img, Codec::store(), Codec::store(), Codec::store(), 0).unwrap();
        let h = Header::read(&bytes).unwrap();
        // File offset of frame 1's u64 size field = first_frame_offset + alignment.
        let rec1 = h.first_frame_offset as usize + h.frame_alignment as usize;
        let bogus = (bytes.len() as u64 + 64).to_le_bytes(); // size past end of body
        bytes[rec1..rec1 + 8].copy_from_slice(&bogus);
        assert!(from_dif(&bytes).is_err(), "serial decode must reject");
        assert!(
            from_dif_workers(&bytes, 4).is_err(),
            "parallel decode must reject and propagate"
        );
    }

    #[test]
    fn render_picks_dark_theme() {
        let img = sample(ColorDepth::Rgba8, IndexWidth::Bit8);
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
            Codec::new(CodecId::Lz4, 9)
        );
        assert!(Codec::parse("nope").is_err());
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = to_dif(
            &sample(ColorDepth::Rgba8, IndexWidth::Bit8),
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
        let mut bytes = to_difr(&sample(ColorDepth::Rgba8, IndexWidth::Bit8)).unwrap();
        bytes[9] = (bytes[9] & !0b11) | 0b10; // index width -> 32-bit
        assert!(matches!(
            from_difr(&bytes),
            Err(DifError::BadIndexWidth(32))
        ));
    }
}
