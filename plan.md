# Modern Diagram Image Format (.dif)

## Motivation
The legacy format GIF contains only 256 colors, while the diagram usually uses less than 256 colors.
Recent website has supported the "dark mode", but the images stay constant. When it comes to the screenshot of terminal or website. The display usually result in hyper-contrast or invisible that makes my eyes sick.

## The Format
Inspired by GIF, use color palette to create mapping. One new thing, color theme -- the named color palette.

### The Mapping

#### UTF-8 Mode
- UTF-8 style variable length color: 128-Color for 1 Byte, 2048-Color for 2B, 65536-Color for 3B, 1112064-Color for 4B.
- The color themes: up to 128 themes. Each color should have the corresponding color (8-bit-RGBA or 16-bit-RGBA) for each theme.
- The default theme is the first theme.
- Color palette size lower bound: 1 (theme) \* 128 (color) \* 4 bytes (8-bit-RGBA) + 128 \* 1 byte = 640 bytes.
- Color palette size upper bound: 128 (themes) \* 1112064 (color) \* 8 bytes (16-bit-RGBA) + 128 \* 1 byte + 1920 \* 2 bytes + 61440 \* 3 bytes + 1048576 \* 4 bytes = 1143136128 bytes = 1.06 GB.

#### Grayscale Mode
- Supports 8-bit and 16-bit grayscale.
- No alpha channel, no palette.

### The frame
- The frame processing is mimicking APNG or GIF.

### The compression
- Lossless compression, need codecs for testing the speed and size.
- Evaluation: $ M = 4 * log(S) - log(C) / 2 - log(D) $, where $S = Size_Original / Size_Compressed$, $C = Memcpy_Speed/Compress_Speed$, $D = Memcpy_Speed/Decompress_Speed$. Higher is better.
- Candidates: libdeflate level 6 (baseline); brotli level 5, 11; bzip3 level 1, 5; kanzi level 1, 2; lz4hc level 4, 9; lz4 fast level 1; lzav level 1; zstd fast level 1; zstd level 3, 10, 22.
- Chosen: zstd level 3 (best, default), lzav level 1 (fast), zstd level 10 (medium-high ratio), lz4 fast level 1 (fastest), brotli level 5 (high ratio), libdeflate level 6 (legacy support), brotli level 11 (extreme high ratio).
- Eliminated: bzip3 (slow), kanzi (unstable), lz4hc (slow on compression), zstd level 22 (slow on compression).

## Implementation

### Benchmarking codec
- A specialized extension `.difr`, the raw dif without any compression.
- The `.drawio` to `.dif` converter should implement the convertion to `.difr` first.

#### Standard files
Get the examples from https://www.drawio.com/docs/diagram-types/ . Need to cite this.

### The Codec of DIF 
See the specifications of The Format.

### The .drawio (diagram-specific xml)  to .dif converter
First generate png with the code from [drawio](https://github.com/jgraph/drawio/tree/v30.0.4).
And then as the general image to `.dif` converter.

### The general image to DIF converter
- Python script for reading the image
- DIF Codec to encode the image into `.dif`

### The Wasm Decoder
- Reuse the codec
- It will respect the "theme" provided by the browser.

### The VS Codium Extension
- The image display should change with respect to the theme, or the mode.

## Evaluation
1. The size comparison with the typical png, loseless jxl, loseless webp, loseless avif.
2. The theme-matching change.
3. The speed of encoding and decoding, with python benchmark code. Compare with gif, png, jxl, webp, avif.
For the existing codecs, use the existing library. Do not reinvent the wheel.

## Checklist

### Format & codec
- [x] DIF core codec — `crates/dif-core/` (`codec.rs`, `format.rs`, `varint.rs`, `error.rs`)
- [x] UTF-8 variable-length color mapping
- [x] Grayscale mode (8/16-bit)
- [x] Color themes / named palette
- [x] Frame model (APNG/GIF-style)
- [x] Format spec — `spec/dif-spec.typ`
- [ ] Spec finalized & matches implementation (cross-check pending)

### Compression study
- [x] Codec benchmark harness — `bench/`
- [x] Metric `M = 4*log(S) - log(C)/2 - log(D)` — `bench/metric.py`
- [x] Per-image TSV + recursive subdir aggregation
- [x] Candidate sweep (15 codec/level configs)
- [x] Decision: chosen / eliminated set
- [x] Two workloads benched — `bench-report-drawio.md` (diagrams), `bench-report-sipi.md` (photos)
- [x] Decision re-validated across both workloads (diagram-target confirmed)

### Converters
- [x] `.drawio` → `.dif` — `dif_tools/drawio.py`
- [x] General image → `.dif` — `dif_tools/convert.py`
- [x] `.difr` raw (uncompressed) path
- [x] Colorspace / themes — `dif_tools/colorspace.py`, `dif_tools/themes.py`
- [x] Python binding — `crates/dif-py/`

### Decode & display
- [x] Wasm decoder — `crates/dif-wasm/`
- [x] Browser respects theme — `web/`
- [x] VS Codium extension — `extension/` (`extension.ts`, `viewer.js`)
- [ ] Extension theme/mode live-switch verified end-to-end

### Evaluation
- [x] Speed bench vs existing codecs — `bench/compare.py`
- [ ] Size comparison vs png / lossless jxl / webp / avif
- [ ] Theme-matching change demo captured
- [ ] Encode/decode speed vs gif/png/jxl/webp/avif written up

### Tests
- [x] `tests/test_codec.py`, `tests/test_convert.py`, `tests/test_bench.py`

## Worklog

- **2026-05-30** — Initial plan (`30588a0`).
- **2026-05-30** — DIF format project scaffold: core codec, converters, wasm, extension, spec (`80d6f4c`).
- **2026-05-30** — Bench: per-image TSV, recursive subdir stats, bzip3 levels, LFS attrs (`462752c`).
- **2026-05-30** — USC-SIPI test images via git-LFS (`b7b2db4`).
- **2026-05-31** — Metric reweighted to `4*log(S) - log(C)/2 - log(D)`; candidate list + chosen/eliminated recorded; verified against `bench-codecs.tsv`. Fixed stale formula docstring in `bench/metric.py`.
- **2026-05-31** — Fixed report writer (`bench/__main__.py`): title/table separation + open file once outside loop (was truncating to last subdir only).
- **2026-05-31** — Re-evaluated codec decision across diagram vs photo workloads. Confirmed metric is workload-sensitive (ratio term dominates → photos go negative, off target). Chosen set holds for the diagram target; `brotli-5` > `zstd-22` at equal ratio (≈69× faster compress); `lzav-1` / `lz4-fast1` robust across both.
