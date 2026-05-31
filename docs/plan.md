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
- Wired into `.dif`: the 7 chosen variants map to (codec byte, level byte) pairs;
  the header carries both. XZ was dropped from the format (LZMA range-coder,
  bzip3-like, weak lzbench position — not a chosen variant); its codec id 3 is now
  reserved/rejected.
- Future candidate (not benched yet): **zxc** — asymmetric lossless, WORM profile
  (heavy encode / very fast decode, >40% faster than LZ4 on ARM64), which fits
  `.dif`'s encode-once/decode-many access pattern. Early stage; record for a future
  sweep. https://github.com/hellobertrand/zxc

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
- [x] Codec `level` byte + 7 variant ids (lz4 + lzav), XZ dropped — format v2
- [x] Spec **structure** cross-checked against the v2 implementation —
  `spec/dif-spec.typ`: relabelled v1→**v2**; fixed the `M` formula to
  `4·log(S) − log(C)/2 − log(D)` (matched `bench/metric.py`); deleted the stale
  XZ cross-library-interop paragraph (XZ removed, id 3 reserved); candidate list
  `zstd fast/3/10/22`. Header/body/varint/codec-table verified against
  `format.rs` + `codec.rs`.
- [x] Spec **Evaluation table dropped** — the hand-picked 800×600 flowchart table
  (DIF 233 B, `rel` normalized to DIF) was stale vs the 245 B v2 demo and
  optimistic (it showed DIF beating WebP). Replaced with prose citing the
  PNG-relative aggregate (`docs/bench-formats-mt.md`): DIF `brotli` ≈×0.47 of PNG
  on diagrams (beats JXL/AVIF/GIF), WebP-ll ≈×0.32 the one smaller competitor.

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
- [x] Dark-theme derivation: achromatic flip + chromatic tone-compress + sRGB gamut map
- [ ] P3 / Rec.2020 gamut targets (sRGB done; needs a format color-space tag + wide-gamut canvas)
- [x] Python binding — `crates/dif-py/`

### Decode & display
- [x] Wasm decoder — `crates/dif-wasm/`
- [x] Wasm decodes all 7 codecs in-browser (zig cross-compile, `wasm32-wasip1`)
- [x] Browser respects theme — `web/`
- [x] VS Codium extension — `extension/` (`extension.ts`, `viewer.js`)
- [x] Extension wasm fixed + build recipes — the committed bundle was a stale
  pre-`wasm-native` wasm-pack build (pure-Rust only, **couldn't decode the
  default zstd-3 `.dif`**). New `just ext-build` reuses the zig/wasip1 decoder
  from `just wasm` (all 7 codecs) + wires `wasi_shim.js` into the webview
  (import map + CSP `cspSource`); `just ext-package` emits a `.vsix` (GUI-install
  in VS Code / VSCodium / Cursor). Dropped the broken `build:wasm` npm script.
  `just ext-build && ext-package` verified clean (`.vsix` 200 KB, 513 KB bundle).
- [x] Extension theme/mode live-switch verified end-to-end — confirmed in
  VSCodium: the `.dif` custom editor opens and re-themes with the editor mode.

### Evaluation
- [x] Speed bench vs existing codecs — `bench/compare.py`
- [x] `bench formats`: per-DIF-codec rows, `.drawio` support (+ render cache),
  live pipe-table stream, markdown report + per-(image,format) TSV
- [x] Encode timed *raw bitmap → file* (build in the timed region; parity with
  `png_encode(arr)`); other formats at library-default effort (avif `speed=6`,
  jxl `effort=7`, webp default)
- [x] **DIF encode build bottleneck fixed** — moved pixel processing into Rust
  (`dif_core::indexed_from_rgba8`, `std`-gated `HashMap` dedup) exposed via
  `dif.Image.indexed_from_rgba8(w,h,depth,rgba_bytes)` + `palette()` /
  `add_indexed_theme()`. Python now hands the raw RGBA8 buffer to native code
  (like `png_encode(arr)`) instead of running `np.unique` over millions of
  pixels and marshalling a per-pixel index list across PyO3. Dark theme still
  derived in Python from the small light palette, then appended.
  Encode **4.8 → ~410 MB/s** (zstd-3/lz4/lzav), now *faster than png*; codec
  ranking clean (brotli-11 the lone slow one). Intermediate numpy fix (pack
  RGBA8→u32, 1-D `np.unique`, ~7× over `axis=0`) is superseded.
- [x] **Refactor** — native-build follow-ups, all landed:
  - [x] Grayscale native: `dif_core::grayscale_from_samples` (+ `dif.Image.
    grayscale_from_samples`) — Python hands the raw sample buffer (u8 / LE-u16)
    like `indexed_from_rgba8`; no more `.tolist()`. Encode off the ~5 MB/s floor
    (→ ~500 MB/s single-theme on a SIPI gray plate).
  - [x] Dark-theme OKLCh derivation **ported to Rust** (`dif_core::derive` via the
    `palette` crate, f64; `derive` feature, encode-only — gated out of the wasm
    decoder). Converter calls `Image.add_dark_theme(strategy)`, so no palette/LUT
    crosses PyO3. 2-theme encode **210 → ~418 MB/s** (parity with single-theme
    ~472; faster than png). `themes.py` `derive_palette`/`derive_lut` are now thin
    wrappers over `dif.derive_dark_palette`/`derive_dark_lut`; `colorspace.py`
    deleted (single source of truth in Rust).
  - [x] `_load` deduped — `load_image` returns the natural dtype (gray-8 → uint8);
    `bench/compare.py` calls it directly. `_to_palette` removed with the old path.
  - `dif.Image.indexed(...)`/`grayscale(...)` list constructors **kept** — only
    `tests/test_codec.py` uses them; the `*_from_*` helpers are the converter paths.
- [x] Multithreaded `.dif` encode (zstd `NbWorkers` / brotli `compress_multi`,
  both live via the `native` feature) **roundtrip-verified** — `bench codecs
  --numthreads N` adds rust `dif-{codec}` / `dif-{codec}-mt` rows that encode
  the real `.dif` container and decode back, so the existing `decomp != raw`
  check covers the mt path (`bench formats` `-mt` rows never checked it: their
  dif rows pass `expected=None`, hardcoding lossless). Confirmed: zstd barely
  splits at diagram body sizes (≈0 size delta); brotli's meta-block split moves
  ratio a little either way; all rows `ok=1`.
- [x] Size comparison vs png / lossless jxl / webp / avif — `docs/bench-formats-mt.md`
  (per-dir aggregate, `rel` vs png). Diagrams: webp-ll smallest (x0.32), `.dif`
  brotli-11 x0.47, gif x0.47 but `LOSSY`. Photos: jxl-ll x0.76, `.dif` weaker
  (ratio term dominates — same workload finding as the codec study).
- [ ] Theme-matching change demo captured
- [x] Encode/decode speed vs gif/png/jxl/webp/avif — same report
  (`docs/bench-formats-mt.md`), enc/dec MB/s columns. `.dif` decode 850–1200
  MB/s (zstd/lz4/lzav), encode 350–600 MB/s at the fast levels (faster than png);
  brotli-11 the lone slow encoder. jxl/avif an order slower to decode.

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
- **2026-05-31** — Format **v2**: header gained a `level:u8` byte after `codec:u8`; added codec ids `Lz4=5` / `Lzav=6` (new `crates/lzav-shim` C shim); dropped XZ (id 3 reserved). Threaded the 7 chosen variant strings through `dif-py` + `bench formats` (`--dif-codecs`); no-arg default → `zstd-3`. Transcoded `web/flowchart.dif` v1→v2.
- **2026-05-31** — Wasm decoder now reads **all 7 codecs in-browser**: `cargo-zigbuild` (+ `ziglang` from uv) cross-compiles the C codecs (zstd, lzav) to **`wasm32-wasip1`** so wasi-libc supplies `malloc`/headers; `web/wasi_shim.js` + an import map stub the 4 unused wasi imports. `just setup-wasm` / `wasm` recipes. Verified headless (node) byte-identical to native.
- **2026-05-31** — Reworked the `arithmetic` dark-theme derivation: achromatic colors flip (`L'=1-L`, white↔black for the background) while chromatic colors keep hue and are tone-compressed into the dark band (so light high-chroma colors like yellow stay a visible muted gold instead of crushing to near-black), then **sRGB gamut-mapped** (OKLCh chroma reduction, not a hard clip). P3/Rec.2020 targets scaffolded but WIP (`NotImplementedError`). Cache-busted the demo `.dif` fetch.
- **2026-05-31** — `bench formats` overhaul. Fixed `.drawio` (it went through `load_image`/PIL directly and crashed — only the DIF row survived): extracted `resolve_raster()` (drawio→PNG with mtime cache reuse, shared by converter + bench) so every format encoder sees the same raster. Output now mirrors `bench codecs`: per-image progress line, live fixed-width **pipe table** (per-format rows stream as measured), GitHub-md report (`--report`) + per-(image,format) **TSV** (`--out`). Per-DIF-codec rows already existed (`--dif-codecs`).
- **2026-05-31** — Made `bench formats` encode honest: timed **raw bitmap → file** (moved the palette/index + dark-theme build into the timed closure via new `dif_image_from_array(arr, …)`, starting from the in-memory array — parity with `png_encode(arr)`, no file I/O timed). Table layout: one **2-theme** headline row (`dif-zstd-3-2t`, the shipped light+dark `.dif`) + **single-theme** codec rows (apples-to-apples with the single-image formats). Other formats pinned to library defaults (avif `speed=6` — imagecodecs otherwise leaves aom at speed 0 ≈0.3 MB/s; jxl `effort=7`; webp default). imagecodecs 2026.5.10 bundles aom-only (no svt/rav1e); system/pyproject installs aren't picked up.
- **2026-05-31** — Found the DIF **encode build bottleneck**: all codecs floor at ~4.8 MB/s. First suspected `.tolist()`/PyO3 marshalling, but the split showed `np.unique(flat, axis=0)` (lexsort over 2.4M RGBA rows) was ~99% (≈1.9 s); `.tolist()`+PyO3 only ~60 ms. `bench codecs` (over the `.difr` body) remains the codec-speed source of truth.
- **2026-05-31** — Killed the bottleneck by moving pixel work to Rust: new `dif_core::indexed_from_rgba8` (`std`-gated `HashMap<u32,u32>` dedup in one pass) + `dif.Image.indexed_from_rgba8` / `palette()` / `add_indexed_theme()`. `dif_image_from_array` now passes the raw RGBA8 buffer to native code (parity with `png_encode(arr)`); the dark theme is derived in Python from the small light palette and appended. Encode **4.8 → ~410 MB/s** (zstd-3/lz4/lzav) — faster than png; codec ranking clean (brotli-11 alone slow at 7.5 MB/s). Grayscale path still uses `.tolist()`; logged refactor TODOs in the checklist.
- **2026-05-31** — Closed the refactor TODOs. (1) **Native grayscale** `dif_core::grayscale_from_samples` (alloc-only, LE-u16) + `dif.Image.grayscale_from_samples`; Python hands the raw sample buffer, no `.tolist()` — gray encode off the ~5 MB/s floor to ~500 MB/s single-theme. (2) **Dark-theme OKLCh derivation ported to Rust** (`crates/dif-core/src/derive.rs`) using the **`palette` 0.7.6** crate (f64, so it reproduces the numpy reference — invert exact, arithmetic within the pinned thresholds). New `derive` feature (`= std + palette`) folded into `native`; **excluded from `wasm-native`** so the browser decoder pulls no `palette` (verified via `cargo tree`). Converter now calls `Image.add_dark_theme(strategy)` (derives + appends entirely native — no palette/LUT crosses PyO3); `themes.py` `derive_palette`/`derive_lut` became thin wrappers over module fns `dif.derive_dark_palette`/`derive_dark_lut`, and `colorspace.py` was deleted (single source of truth in Rust). 2-theme encode **~210 → ~418 MB/s** (parity with single-theme ~472, still > png). (3) Deduped `_load`  <!-- early-out below pushed 2-theme higher --> — `load_image` returns the natural dtype (gray-8 → uint8), `bench/compare.py` uses it directly; `_to_palette` gone. `indexed`/`grayscale` list ctors kept for `test_codec.py`. Verified: `cargo test --features native` (20, +5 derive), `clippy --all-features` clean, `pytest` 34 green.
- **2026-05-31** — Dark-derive gamut **early-out**: skip the 25-iter OKLCh chroma search when the color already fits sRGB (every gray/achromatic color + most tone-compressed ones). Output byte-identical (the in-gamut branch already converged to `k=1`); removed the two dead binding methods `Image.palette`/`add_indexed_theme` (+ stub). 2-theme encode: **grayscale 142 → ~464 MB/s** (256-entry LUT is all in-gamut, search fully skipped), **diagram 417 → ~459** (vs ~481 single-theme). Confirmed the observation that 2-theme size-diff and speed-diff both scale with palette size N (extra stored palette ∝ N, derivation cost ∝ N).
- **2026-05-31** — Closed an **unverified mt path**. `bench formats` drives the rust multithreaded encode (`to_dif_workers` → zstd `NbWorkers` / brotli `compress_multi`) for its `-mt` rows but never checks the decode matches the input — every dif row passes `expected=None`, so `lossless` is hardcoded `True`. `bench codecs` has the roundtrip check (`decomp != raw`) but had no mt. Added `bench.codecs.dif_codecs(numthreads)`: rust-`dif`-backed codecs that compress the `.difr` body through the real `.dif` container and decode back, so the mt encode path is exercised **and** roundtrip-verified by the existing check. Empty unless `--numthreads > 1` (default run unchanged); each codec yields a single-thread reference (`dif-{c}`) + worker variant (`dif-{c}-mt`) so the worker size delta shows side by side. Verified one image per dir (tiff + drawio, `--numthreads 4`): all `dif-*`/`-mt` rows `ok=1`; zstd delta ≈0 (body too small to split), brotli moves a little either way — matches the `codec.rs` comments. Moved `plan.md` + `plan-format-codecs.md` to `docs/`; `*.tsv` now tracked via git-LFS.
- **2026-05-31** — **Extension wasm fix + build recipes.** The committed `extension/media/pkg` was a stale pre-`wasm-native` wasm-pack bundle (225 KB, pure-Rust store/deflate/brotli/lz4) that **could not decode the format's default zstd-3 `.dif`** — and `package.json`'s `build:wasm` (wasm-pack on wasm32-unknown-unknown) can no longer build dif-wasm at all, since it now pulls the C codecs. Added `just ext-build` (reuses the zig/wasip1 decoder from `just wasm` — all 7 codecs — stages it + `wasi_shim.js` into `media/`, `pnpm install`, `tsc`) and `just ext-package` (`@vscode/vsce` → `.vsix`, GUI-installable in VS Code / VSCodium / Cursor). Wired the wasi shim into the webview (`extension.ts`: import map + CSP `cspSource`, mirroring `web/`), dropped the dead `build:wasm` script, added `extension/README.md` + `.gitignore` (the staged bundle/shim/`out`/`.vsix` are generated, sources are `crates/dif-wasm` + `web/wasi_shim.js`). New bundle 525 KB, carries `zstd`/`lzav`, byte-identical to `web/pkg`. Live theme-switch verified in VSCodium.
- **2026-05-31** — **Spec structural cross-check** (`spec/dif-spec.typ`). Relabelled v1→**v2**; corrected the `M` formula to `4·log(S) − log(C)/2 − log(D)` (was the superseded `log(5S/4) − log(C/4) − log(D)`; now matches `bench/metric.py`); deleted the stale paragraph claiming XZ cross-library interop (XZ removed in v2, id 3 reserved — the text contradicted the codec table); candidate list `zstd fast/3/10/22`; dropped a lingering "v1" in the evaluation note. Header/body/varint/codec-table re-verified against `format.rs` + `codec.rs`; `typst compile` clean. **Held back** the "spec final" claim: the Evaluation example table (DIF 233 B, `rel` normalized to it) is stale vs the 245 B v2 demo — needs regenerated all-format numbers first.
