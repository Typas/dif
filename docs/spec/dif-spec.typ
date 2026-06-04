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
  #text(11pt)[Specification v3 · ADSP final project]
]

#outline(depth: 2, indent: auto)
#v(4pt)

= Motivation

GIF caps a palette at 256 colors, yet most diagrams use far fewer; meanwhile a
static raster cannot adapt to a host's light/dark theme, so terminal and website
screenshots become hyper-contrast or nearly invisible under the "wrong" mode.

*DIF* is a lossless, theme-aware, palette-indexed raster format. Inspired by
GIF's palette, it adds a *theme*: a named palette variant tagged with the host
appearances it can display under. One file stores several themes
(light / dark / high-contrast); the decoder renders the one matching the host
appearance and background.

v3 replaces v2's UTF-8-style variable-length index with a *constant-width* index
plane, drops the separate grayscale mode (everything is indexed), and splits the
body into a *two-stage codec* so a decoder can inflate one palette or one frame on
demand. The container header is a fixed 64-byte block.

= Containers

Two serializations share one 64-byte header (#ref(<header>)) and one body
(#ref(<body>)):

- `.difr` — raw, magic `DIFR3`. Every section's codec is `Store`; used to
  benchmark codecs on the uncompressed body.
- `.dif`  — compressed, magic `DIF3`.

== Header <header>

All multi-byte integers are little-endian.

#table(
  columns: (auto, auto, auto, auto),
  align: (right, left, left, left),
  table.header([*off*], [*field*], [*type*], [*meaning*]),
  [0],
  [`magic`],
  [8 B],
  [`DIF3\0\0\0\0` or `DIFR3\0\0\0` (carries version `3`)],

  [8], [`codec`], [u8], [outer whole-body codec (4 b family · 4 b level index)],
  [9],
  [`flags`],
  [u8],
  [bit 0–1 index width; bit 2–5 color depth; 6–7 reserved],

  [10], [`codec_palette`], [u8], [palette-section codec],
  [11], [`codec_frame`], [u8], [per-frame-section codec],
  [12], [`theme_count`], [u8], [stored as `count − 1` (so 1…256 themes)],
  [13], [`reserved`], [u8], [],
  [14], [`frame_count`], [u16], [],
  [16], [`replay_count`], [u16], [0 = infinite, 1 = static],
  [18], [`reserved`], [u16], [],
  [20], [`width`], [u32], [],
  [24], [`height`], [u32], [],
  [28], [`frame_long_offset`], [u32], [upper 32 bits of the first-frame offset],
  [32], [`frame_offset`], [u64], [lower 64 bits of the first-frame offset],
  [40], [`frame_alignment`], [u64], [per-frame stride (multiple of 16)],
  [48], [`index_count`], [u64], [palette length (color count)],
  [56], [`palette_size`], [u64], [compressed palette-section length],
  [64], [`compressed_body[]`], [], [outer-compressed intermediate body],
)

The first-frame offset is `frame_long_offset · 2^64 + frame_offset`, measured from
the file start into the (outer-decompressed) intermediate body. `palette_size`
bounds the single compressed palette blob so the decoder need not rely on a
self-terminating codec.

== Flags

#table(
  columns: (auto, auto, auto),
  align: (left, left, left),
  table.header([*bits*], [*field*], [*values*]),
  [0–1],
  [index width],
  [`00` = 8-bit, `01` = 16-bit, `10` = 32-bit, `11` = 64-bit],

  [2–5], [color depth], [`0x0` = RGBA8, `0x1` = RGBA16],
  [6–7], [reserved], [must be 0],
)

The index width is the smallest that holds `index_count` indices. 32- and 64-bit
are defined but unimplemented in the reference codec (rejected on decode).

== Codec byte

A codec byte packs a 4-bit *family* and a 4-bit *level index*:

#table(
  columns: (auto, auto, auto),
  align: (right, left, left),
  table.header([*family*], [*codec*], [*level index → level*]),
  [0], [common-pick], [benchmark-derived presets; `0` = Store],
  [1], [DEFLATE], [`1…12`],
  [2], [Brotli], [`0…11`],
  [3], [zxc], [`1…6` (1 fastest … 6 densest)],
  [4],
  [Zstandard],
  [`−7, −5, −3, −1, 1, 2, 3, 6, 8, 10, 12, 14, 16, 18, 20, 22`],

  [5],
  [LZ4],
  [fast accel (neg) `−512, −256, −128, −64, −32, −16, −8, −4, −2, −1`, then HC
    (pos) `2, 4, 6, 9, 10, 12`],

  [6], [LZAV], [`1, 2`],
)

Family 0 (`common-pick`) is an indirection: the level index selects a
benchmark-chosen `(family, level)` preset, resolved identically on encode and
decode; `0/0` is `Store` (no compression). The level index otherwise selects a
per-family level from the table above. Decode is *level-agnostic* — every
supported codec's stream is self-describing, so the decoder only needs the family
and the known raw length.

= Body layout <body>

The body has two representations. The *fully-decompressed body* is the indexed
form (palettes + index planes); the *intermediate body* compresses the palette
and each frame section independently and is what the outer `codec` wraps.

- Encoding: raw pixels → index (palette + frames) → per-section compress →
  intermediate body → outer compress → `compressed_body`.
- Decoding: `compressed_body` → outer decompress → intermediate body → per-section
  decompress → indexed form → render.

With the outer codec set to `Store`, the header offsets address frames directly in
the file, so a decoder can inflate one palette or one frame without touching the
rest (low memory; the per-frame 16-byte alignment also enables parallel decode).

== Intermediate body

```text
themes[theme_count]:  abilities:u8  base_color: R,G,B (u8×3)
post_theme_padding:   align to 16 B
compressed_palettes[]: one stream — all palettes concatenated, then compressed
post_palette_padding: align to 16 B
frames[frame_count]:
  size:u64             (size field start → end of compressed_content)
  delay:u32            (microseconds; 0 = static)
  compressed_content[]
  padding:             pad the record to frame_alignment
```

The palette section is a single compressed stream of every theme's palette
concatenated (`palettes[theme][index]`, RGBA at the color depth); a decoder
inflates it sequentially and may stop once the target theme's palette is reached.
Each frame is compressed independently and padded to the uniform
`frame_alignment` stride, so frame `j` starts at `first_frame_offset + j ·
frame_alignment`.

== Fully-decompressed body

```text
themes[theme_count]:  abilities:u8  base_color: R,G,B (u8×3)
post_theme_padding:   align to 16 B
palettes[theme_count][index_count]:  RGBA8 or RGBA16
post_palette_padding: align to 16 B
frames[frame_count]:
  delay:u32
  index_plane[width × height]:  u8 or u16   (row-major)
  padding:              align to 16 B
```

== Offsets

Let `t` = theme_count, `c` = index_count, `s` = color bytes (4 or 8), `i` = index
bytes (1 or 2), `w` = width, `h` = height, and let `align(n)` round up to 16.

- themes: `align(4 · t)` bytes.
- palette section begins at `64 + align(4 · t)`.
- first frame begins at the header's first-frame offset.
- in the fully-decompressed body, palette `k` begins at `64 + align(4·t) + k·c·s`,
  and frame `j` at `64 + align(4·t) + align(t·c·s) + j · align(4 + w·h·i)`.

= Theme model

Each theme carries an `abilities` byte and a `base_color` (RGB8). The abilities
bits mark which host appearances the theme can display under:

#table(
  columns: (auto, auto),
  align: (left, left),
  table.header([*bit*], [*capability*]),
  [0], [light],
  [1], [dark],
  [2], [high-contrast],
  [3–7], [reserved (must be 0)],
)

== Theme picking

The decoder is given the host appearance (e.g. from
`matchMedia('(prefers-color-scheme: dark)')` or the editor's color-theme kind) and
the host background color. Among the themes whose abilities cover that appearance,
it picks the one whose `base_color` is nearest the host background (squared RGB
distance), ties resolving to the lowest index; if no theme is capable it falls
back to theme 0.

How a single-theme source *generates* its alternate themes (e.g. an OKLab dark
derivation) is an *encoder implementation detail* and is not part of this format —
the format only specifies how themes are stored and picked.

= Index encoding

Palette indices form a constant-width plane: each index is a `u8` (8-bit width) or
a little-endian `u16` (16-bit width), row-major. The encoder picks the smallest
width that holds `index_count` colors (≤ 256 → 8-bit, ≤ 65536 → 16-bit). A fixed
width makes the plane trivially seekable and SIMD-friendly, and once the codec
layer runs it is no larger than the v2 varint stream in practice.

= Compression codecs <codecs>

`dif-core` is `no_std` + `alloc` by default, exposing the portable, pure-Rust set
— Store, DEFLATE, LZ4 — which decodes in the WebAssembly build with no native
dependencies. The `std` feature adds Brotli (pure Rust, wasm-decodable). The
`native` feature adds Zstandard, a `libdeflate` encoder, LZAV, and zxc (C); zstd
and LZAV reach the browser decoder when cross-compiled with `zig` (the
`wasm-native` build), but zxc is native-only (its bindings don't cross-build to
wasm), so a zxc `.dif` decodes only on the host. A heap allocator is always
required.

The palette and each frame may use a different codec (`codec_palette`,
`codec_frame`), and the whole intermediate body is wrapped by the outer `codec`;
all three are codec bytes from #ref(<header>).

== Codec benchmark and the $M$ metric

To choose a codec, candidates are measured over the `.difr` body against a
`memcpy` baseline:

$ M = 4 log(S) - log(C) \/ 2 - log(D) $

where $S = "size"_"orig" \/ "size"_"comp"$ (ratio, higher better),
$C = "memcpy"_"speed" \/ "compress"_"speed"$, and
$D = "memcpy"_"speed" \/ "decompress"_"speed"$ (both slowdowns, lower better).
Higher $M$ is better. Candidates: libdeflate L6 (baseline), Brotli 5/11,
bzip3 @bzip3, kanzi 1/2 @kanzi, lz4hc 4/9 and lz4 fast @lz4, lzav @lzav,
zstd fast/3/10/22 @zstd. kanzi and lzav are written in C/C++ and exposed to the
Python harness as `ctypes` C-ABI shared libraries.

= Evaluation

Lossless size and speed are compared against PNG, lossless JXL/WebP/AVIF and GIF
via `imagecodecs` (`bench formats`). Sizes are reported relative to PNG and
aggregated over a diagram and a photo corpus.

On true flat-color diagrams — the target use case — DIF is highly competitive:
its `brotli`-compressed body lands around ×0.47 of PNG, beating JXL (≈×0.59) and
AVIF (≈×1.18) and edging GIF (≈×0.47, but GIF is lossy above 256 colors).
Lossless WebP is the one consistently smaller competitor (≈×0.32). On
anti-aliased or photographic images that same prediction-free design makes DIF
trail PNG/WebP/JXL; the 16-bit index ceiling also caps DIF at 65 536 colors.

= Implementation map

/ `crates/dif-core`: Rust codec — 64-byte container, constant-width index plane,
  two-stage body, theme picking, compression. Unit-tested for lossless
  round-trips across the RGBA8/RGBA16 × 8/16-bit-index matrix.
/ `crates/dif-py`: PyO3/maturin bindings exposing the `dif` module.
/ `crates/dif-wasm`: `wasm-bindgen` decoder reusing `dif-core`.
/ `crates/kanzi-shim`, `crates/lzav-shim`: C-codec shims for the benchmark.
/ `py/dif_tools/`: image→DIF and drawio→PNG→DIF converters and the (encoder-side)
  dark-theme derivation strategies.
/ `py/bench/`: the $M$-metric codec harness and cross-format comparison.
/ `web/demo/`, `web/extension/`: browser demo and a theme-aware VSCodium editor.

Standard diagram samples should be taken from the draw.io documentation
@drawio; drawio rendering uses draw.io desktop (pinned to v30.0.4).

#bibliography("refs.bib", title: "References")
