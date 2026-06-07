# DIF --- Modern Diagram Image Format (`.dif`)

A palette-based image format built for **diagrams** and **theme-aware
display**. Like GIF it maps pixels through a color palette, but each palette is
tagged with the host appearances it can display under (`light` / `dark` /
`high-contrast`), so a single file re-themes itself to match the browser or
editor instead of staying a fixed bitmap. The index plane is constant-width
(8/16/32/64-bit, 32/64-bit not implemented), and the mapped color are RGBA8/RGBA16. APNG/GIF-style frames are
supported, and the `.dif` body uses a two-stage codec (per-palette + per-frame
sections under an outer pass) drawn from a study-chosen lossless set.

## Format spec
- [`dif-spec.pdf`](docs/spec/dif-spec.pdf)

## Prerequisites

- [`uv`](https://docs.astral.sh/uv/) --- all Python tasks (project pins 3.12).
- Rust toolchain (`cargo`) --- the core and bindings.
- [`just`](https://github.com/casey/just) --- task runner (recipes below).
- `git-lfs` --- images and `*.tsv` reports are LFS-tracked.
- Optional: `podman` + `pnpm` (`.drawio` rendering / extension), `typst` (spec),
  a C compiler (`cc`) and network (benchmark `lzav`/`kanzi`/`bsc` shims),
  a CUDA compiler (`nvcc`) and OMP library (`libomp`) for GPU encoding,
  [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov#installation) for code coverage.

## Quick Start

### Initialization
```sh
# Sync the submodules
git submodule sync && git submodule update --init
```

### Build VS Code/Codium Extension Package
```sh
# Build wasm module
just wasm-setup   # one-time toolchain

# Build the extension
just ext-package
```

#### Install Package 
```sh
just ext-install [code/codium/cursor/...]
```

The installation is default on `code`.
The `.vsix` also installs through the GUI (Extensions |> **Install from VSIX...**) in any VS Code-family editor.

### Convert images to DIF format
```sh
just py # ensure dif converter built

just convert <in> <out> [threads [index_width [frame_codec [palette_codec [outer_codec [strategy]]]]]]
```

The default threads is 1 and the default index_width is auto (can be `8` and `16`).
The default configuration of `(outer, palette, frame)` codecs is `(store, zstd-16, zstd-10)`.
The default strategy is `arithmetic`.

### Run a Benchmark
#### Initialization
```sh
just py # ensure dif python kit built

just bench-setup [--cuda] # setup benchmark
```

#### Benchmark Codecs
```sh
just bench-codecs data/testdata/ [options]
```

See `bench-codecs` part to see the option details.

#### Benchmark between Formats
```sh
just bench-formats data/testdata/ [options]
```

See `bench-formats` part to see the option details.

### Start a Demo Web Server
```sh
```

## justfile recipes

A bare `just` (or `just --list`) prints every recipe.

### `.drawio` rendering
| Recipe              | Does                                                                           |
|---------------------|--------------------------------------------------------------------------------|
| `drawio-setup`      | Pull the `rlespinasse/drawio-export` container + `pnpm install` the extension. |
| `drawio-png IN OUT` | Render a `.drawio` to PNG via the local container (no diagrams.net).           |

## Benchmarks (`just bench-*`)

Two distinct studies, on two different codec layers. Both take image files or a
directory (recursively expanded), stream a live table per image, and write a
per-image **TSV** (`--out`) plus a per-directory aggregate **report** (`--report`).

### `bench-setup`
```sh
just bench-setup
```
Builds the optional native benchmark codecs that have no PyPI wheel: the **lzav**
shim (fetches `lzav.h`, compiles with `cc`) and the **kanzi** shim (vendors
kanzi-cpp, `cargo build`). Needs a compiler and network; missing ones are just
reported as unavailable, the rest of the harness still runs.

### `bench-codecs` --- rank codecs over the `.difr` body
```sh
just bench-codecs data/testdata/                  # whole tree
just bench-codecs img.png --repeats 5 --strategy arithmetic
just bench-codecs data/testdata/ --num-threads 4   # adds the rust mt probe
```
Encodes each image to a raw `.difr` body, then compresses that body with **every
registered codec** (deflate, brotli, bzip3, lz4/lz4hc, zstd, lzav, kanzi, bsc) and ranks them by the metric

> `M = 4*log(S) - log(C)/2 - log(D)` --- higher is better,
> where S = original/compressed size, C = memcpy/compress speed, D =
> memcpy/decompress speed (speeds normalised to a memcpy baseline).

Every codec's output is **roundtrip-checked** (`decompress(compress(x)) == x`);
a mismatch marks the row failed instead of crashing the run.

Options: `--strategy` (dark-theme synthesis: `arithmetic`/`invert`/`keep`, default
`arithmetic`) * `--repeats` (timing reps, default 5) * `--num-threads N` (default 1;
**>1** adds rust `dif-{codec}` / `dif-{codec}-mt` rows that drive the real `.dif`
multithreaded encode --- zstd `NbWorkers`, brotli `compress_multi` --- through the
roundtrip check, the only place that mt path is verified) * `--out`
(`bench-codecs.tsv`) * `--report` (`bench-report.md`).

### `bench-formats` --- compare `.dif` against other image formats
```sh
just bench-formats data/testdata/
just bench-formats data/testdata/ --num-threads 4  # parallel jxl/avif/brotli + dif -mt rows
just bench-formats img.png --outer-codecs=zstd,3/brotli,5,11
```
Renders each input (including `.drawio` -> PNG once, cached) to a common raster,
then encodes it as **PNG, lossless WebP/JXL/AVIF, GIF**, and `.dif` at several
codec variants --- reporting size (relative to PNG), encode/decode MB/s, and a
losslessness flag. PNG is the `rel` baseline.

**Choosing codecs (lzbench `-e` syntax).** `--outer-codecs` takes one string where
`/` separates codecs and `,` enumerates levels of the preceding family --- so
`--outer-codecs=zstd,3/brotli,5,11/store` runs `zstd-3`, `brotli-5`, `brotli-11`,
`store`. A bare family with no comma is its default level (`zstd`, `store`);
`family,L1,L2` is that family at each level (`lz4,fast1,hc10`, `zstd,3,-7`). A
`.dif` carries three codec bytes --- outer (whole body), palette section, frame
section --- set independently:

```sh
# outer only; palette + frame inherit the outer codec (the common case)
just bench-formats img.png --outer-codecs=zstd,3/brotli,11

# pin all three sections; each flag is the same /,-syntax. Lists cross-multiply:
#   --outer-codecs=A/B  --palette-codecs=P  -> {A*P, B*P} rows
just bench-formats img.png \
  --outer-codecs=zstd,10 --palette-codecs=store --frame-codecs=zstd,10/lzav,1
```

Levels available per family (the DIF codec table): `deflate` `1,2,3,...,6,...,12`, `brotli`
`0,1,2,...,11`, `bsc` `1,2,3`, `zstd` `-7,-5,-3,-1,1,2,3,6,8,10,12,14,16,18,20,22`, `lz4`
fast `fast1,fast2,fast4,...,fast512` / HC `hc2,hc3,...,hc9,hc10,hc11,hc12`, `lzav` `1,2`, plus `store`.

Options: `--repeats` (default 3) * `--num-threads N` (default 1 = a 1-core
comparison; **>1** scales jxl/avif/brotli encode and adds `dif-{codec}-mt` rows)
* `--outer-codecs` / `--palette-codecs` / `--frame-codecs` (lzbench-syntax
codec specs; palette/frame default to inheriting the outer codec) * `--index-width`
(`/`-separated `auto`/`8`/`16`; one dif row set per width, default `auto`) * `--out`
(`bench-formats.tsv`) * `--report` (`bench-formats.md`).

> [!NOTE]
> `bench-formats` does **not** roundtrip-check its `dif-*` rows (it reports
> them lossless by construction); use `bench-codecs --num-threads N` to actually
> verify the mt encode path. See [`docs/bench-formats-mt.md`](docs/bench-formats-mt.md)
> for a committed sample report.
