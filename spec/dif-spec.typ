#set document(
  title: "DIF — Diagram Image Format Specification",
  author: "Typas Liao",
)
#set page(numbering: "1", margin: 2.2cm)
#set text(font: "New Computer Modern", size: 10.5pt)
#set heading(numbering: "1.1")
#show raw.where(block: true): it => block(
  fill: luma(245),
  inset: 8pt,
  radius: 3pt,
  width: 100%,
  it,
)

#align(center)[
  #text(17pt, weight: "bold")[DIF — The Diagram Image Format]
  #v(2pt)
  #text(11pt)[Specification v1 · ADSP final project]
]

#outline(depth: 2, indent: auto)
#v(4pt)

= Motivation

GIF caps a palette at 256 colors, yet most diagrams use far fewer; meanwhile a
static raster cannot adapt to a host's light/dark theme, so terminal and website
screenshots become hyper-contrast or nearly invisible under the "wrong" mode.

*DIF* is a lossless, theme-aware raster format. Inspired by GIF's palette, it
adds a *theme*: a named palette variant. One file stores several themes
(light / dark / high-contrast); the decoder renders the one matching the host
appearance, falling back to the first (which is byte-exact with the source).

= Containers

Two serializations share one *body* (#ref(<body>)):

- `.difr` — raw, magic `DIFR`. Used to benchmark codecs on the uncompressed body.
- `.dif`  — compressed, magic `DIF1`:

```text
magic:"DIF1"  version:u8  codec:u8  raw_len:u64-LE   compressed_body[]
```

`codec` selects the decompressor (#ref(<codecs>)); `raw_len` is the body length
before compression. All multi-byte integers are little-endian.

= Body layout <body>

```text
flags:u8     bit0 mode (0=indexed, 1=grayscale)   bit1 depth (0=8-bit, 1=16-bit)
width:u32    height:u32    frame_count:u32    theme_count:u8        (1..=128)
themes[theme_count]:     tag:u8 (0=light,1=dark,2=high-contrast)  name_len:u8  name[name_len] (UTF-8)
frame_delays[frame_count]: u16   (milliseconds; 0 = static)

indexed:     color_count:varint
             palette[theme][color]:  R,G,B,A   (each channel 1 byte if 8-bit, else 2 bytes LE)
             frames[frame][pixel]:   varint index           (row-major)

grayscale:   lut[theme][level]:      sample (1 or 2 bytes)   (level in 0 .. 2^depth)
             frames[frame][pixel]:   sample (1 or 2 bytes)   (row-major)
```

== Modes

Both modes are *per-theme*, and `sample_depth` (the single `flags` bit) governs
both palette channel depth and grayscale sample depth.

/ indexed: a per-theme RGBA palette plus a variable-length index stream. This is
  PNG/GIF "indexed-color", extended with one palette per theme.
/ grayscale: raw samples plus a per-theme 1-D *tone LUT* mapping a stored sample
  to the themed sample. The first theme's LUT is the identity, so the source is
  reproduced exactly; an alternate theme can lift a near-black gray so it stays
  visible on a dark background. LUTs are small and monotonic, so the codec layer
  compresses them to almost nothing.

== Variable-length index encoding <varint>

Palette indices are packed with a UTF-8-inspired scheme: the byte-length
thresholds are exactly UTF-8's, so an index is stored the way a Unicode scalar
would be.

#table(
  columns: (auto, auto, auto),
  align: (left, left, left),
  table.header([*value range*], [*bytes*], [*bit pattern*]),
  [`0 ..= 127`], [1], [`0xxxxxxx`],
  [`128 ..= 2047`], [2], [`110xxxxx 10xxxxxx`],
  [`2048 ..= 65535`], [3], [`1110xxxx 10xxxxxx 10xxxxxx`],
  [`65536 ..= 2097151`], [4], [`11110xxx 10xxxxxx 10xxxxxx 10xxxxxx`],
)

The 4-byte form's 21 bits cover the format's 1,112,064-color ceiling (the count
of valid Unicode scalars).

= Theme model

Each theme carries a `tag` (light / dark / high-contrast) and a UTF-8 name.
At decode time the host passes its appearance — from
`matchMedia('(prefers-color-scheme: dark)')` in a browser, or the editor color
theme kind in VSCodium — and the decoder selects the first theme whose tag
matches, else theme 0.

== Theme generation

A single-theme source synthesizes its alternate theme(s) at conversion time.
Three strategies (the source theme is always the lossless identity):

/ keep: alternate theme identical to the source (theme-agnostic).
/ invert: photographic negative, $"out" = max - "value"$ per channel (grayscale:
  $max - "sample"$).
/ arithmetic: perceptual lightness inversion in OKLab @oklab — convert
  sRGB→linear→OKLab, set $L' = 1 - L$, convert back — preserving hue and chroma.
  Grayscale uses the same transform per LUT level.

= Compression codecs <codecs>

A `.dif` names its codec by id. `dif-core` is `no_std` + `alloc` by default,
exposing the portable, pure-Rust set — store, DEFLATE, XZ (ids 0, 1, 3) — which
decodes in the WebAssembly build with no native dependencies. Two Cargo features
add the rest. The `std` feature adds Brotli (id 2): it is pure-Rust but its
streaming encoder/decoder need the standard library, which wasm provides, so it
stays wasm-decodable. The `native` feature adds Zstandard (C-linked `zstd-safe`,
id 4) plus a faster liblzma XZ encoder, and is unavailable in wasm. A heap
allocator is always required; a `no_std` host must install a `#[global_allocator]`.

#table(
  columns: (auto, auto, auto, auto, auto),
  table.header([*id*], [*codec*], [*library*], [*feature*], [*wasm*]),
  [0], [store (raw)], [—], [default], [yes],
  [1], [DEFLATE], [`miniz_oxide`], [default], [yes],
  [2], [Brotli], [`brotli` (pure Rust)], [`std`], [yes],
  [3],
  [XZ],
  [`lzma-rust2` (decode); `xz2`/liblzma encode under `native`],
  [default],
  [yes],

  [4], [Zstandard], [`zstd-safe`], [`native`], [no],
)

XZ is interoperable across libraries: a `.dif` encoded by liblzma (`xz2`) decodes
correctly with the pure-Rust `lzma-rust2` reader used in wasm, and vice versa.

== Codec benchmark and the $M$ metric

To choose a codec, candidates are measured over the `.difr` body against a
`memcpy` baseline:

$ M = log(5 S \/ 4) - log(C \/ 4) - log(D) $

where $S = "size"_"orig" \/ "size"_"comp"$ (ratio, higher better),
$C = "memcpy"_"speed" \/ "compress"_"speed"$, and
$D = "memcpy"_"speed" \/ "decompress"_"speed"$ (both slowdowns, lower better).
Higher $M$ is better. Candidates: libdeflate L6 (baseline), Brotli 5/11,
bzip3 @bzip3, kanzi 1/2 @kanzi, lz4hc 4/9 and lz4 fast @lz4, lzav @lzav,
zstd fast/3/22 @zstd. kanzi and lzav are written in C/C++ and exposed to the
Python harness as `ctypes` C-ABI shared libraries (kanzi via the Rust crate
`crates/kanzi-shim` wrapping kanzi-cpp's C API).

= Evaluation

Lossless size and speed are compared against PNG, lossless JXL/WebP/AVIF and GIF
via `imagecodecs`. On true flat-color diagrams — the target use case — DIF wins
decisively; for example an 800×600, 5-color flowchart:

#table(
  columns: (auto, auto, auto),
  table.header([*format*], [*size (bytes)*], [*relative*]),
  [DIF (brotli)], [233], [×1.00],
  [WebP lossless], [256], [×1.10],
  [JXL lossless], [388], [×1.67],
  [AVIF lossless], [721], [×3.09],
  [PNG], [3498], [×15.0],
  [GIF], [5313], [×22.8],
)

On anti-aliased or photographic images DIF trails PNG/WebP/JXL, since v1 stores
the index stream without spatial prediction; this is the natural next extension.

= Implementation map

/ `crates/dif-core`: Rust codec — format, varint, indexed/grayscale encode and
  decode, theme selection, compression container. Unit-tested for lossless
  round-trips.
/ `crates/dif-py`: PyO3/maturin bindings exposing the `dif` module.
/ `crates/dif-wasm`: `wasm-bindgen` decoder reusing `dif-core`.
/ `crates/kanzi-shim`: Rust cdylib wrapping kanzi-cpp for the benchmark.
/ `dif_tools/`: image→DIF and drawio→PNG→DIF converters and theme strategies.
/ `bench/`: the $M$-metric codec harness and cross-format comparison.
/ `web/`, `extension/`: browser demo and a theme-aware VSCodium custom editor.

Standard diagram samples should be taken from the draw.io documentation
@drawio; drawio rendering uses draw.io desktop (pinned to v30.0.4).

#bibliography("refs.bib", title: "References")
