# DIF — Modern Diagram Image Format (`.dif`)

A lossless, palette-based image format built for **diagrams** and **theme-aware
display**. Like GIF it maps pixels through a colour palette, but each colour
carries one entry *per named theme* (e.g. `light` / `dark`), so a single file
re-themes itself to match the browser or editor instead of staying a fixed
bitmap. Grayscale (8/16-bit) and APNG/GIF-style frames are supported, and the
compressed `.dif` body uses one of 7 study-chosen lossless codecs.

- Format spec: [`spec/dif-spec.typ`](spec/dif-spec.typ)
- Design + worklog: [`docs/plan.md`](docs/plan.md), [`docs/plan-format-codecs.md`](docs/plan-format-codecs.md)

## Repo layout

| Path                                      | What                                                                                             |
|-------------------------------------------|--------------------------------------------------------------------------------------------------|
| `crates/dif-core/`                        | Rust core: container (`codec.rs`), raw layout (`format.rs`), dark-theme derivation (`derive.rs`) |
| `crates/dif-py/`                          | Python extension (`dif`), built with maturin                                                     |
| `crates/dif-wasm/`                        | Browser decoder (wasm)                                                                           |
| `crates/lzav-shim/`, `crates/kanzi-shim/` | C-codec shims for the benchmark                                                                  |
| `dif_tools/`                              | Python converters: image/`.drawio` → `.dif`                                                      |
| `bench/`                                  | Codec + format benchmark harness                                                                 |
| `web/`                                    | In-browser viewer (theme-aware)                                                                  |
| `extension/`                              | VS Codium extension                                                                              |
| `spec/`                                   | Typst format spec                                                                                |
| `testdata/`                               | Sample diagrams + photos (git-LFS)                                                               |

## Prerequisites

- [`uv`](https://docs.astral.sh/uv/) — all Python tasks (project pins 3.12).
- Rust toolchain (`cargo`) — the core and bindings.
- [`just`](https://github.com/casey/just) — task runner (recipes below).
- `git-lfs` — `testdata/` images and `*.tsv` reports are LFS-tracked.
- Optional: `podman` + `pnpm` (`.drawio` rendering / extension), `typst` (spec),
  a C compiler (`cc`) and network (benchmark `lzav`/`kanzi` shims).

## Quick Start

```sh
# 1. Build the Rust core and run its tests (portable no_std tier).
just test

# 2. Build the Python extension `dif` into the uv env (needed by the
#    converter, the benchmarks, and pytest).
just py

# 3. Convert an image (or a .drawio) to a themed .dif.
uv run python -m dif_tools convert testdata/usc-sipi-misc/4.1.01.tiff out.dif
#   --codec zstd-3|zstd-10|brotli-5|brotli-11|lz4-fast1|lzav-1|libdeflate-6|store
#   --theme-strategy arithmetic|invert|keep   (how the dark theme is derived)
#   --raw                                     (write uncompressed .difr instead)

# 4. Run the Python test suite.
just pytest

# 5. (Optional) View in the browser, theme-aware.
just setup-wasm   # one-time toolchain
just wasm         # build the decoder into web/pkg
#   then serve web/ and open it; the page picks light/dark from the browser.
```

A bare `just` (or `just --list`) prints every recipe.

## justfile recipes

### Rust core (`dif-core`)
| Recipe              | Does                                                                                 |
|---------------------|--------------------------------------------------------------------------------------|
| `build`             | Build the no_std+alloc default (store/deflate/lz4) — also the portability check.     |
| `build-std`         | Build with `std` (adds brotli).                                                      |
| `build-native`      | Build with `native` (brotli + zstd + libdeflate encoder + lzav, + mt + dark-derive). |
| `check-nostd`       | Assert the portable core still builds no_std (alias of `build`).                     |
| `test`              | Test the core (store/deflate/lz4 under `cfg(test)`).                                 |
| `test-native`       | Test with all native codecs.                                                         |
| `test-all`          | Full matrix: every feature tier builds, both test sets pass.                         |
| `fmt` / `fmt-check` | `cargo fmt` (write / check).                                                         |
| `clippy`            | Clippy with `--all-features -D warnings`.                                            |

### Bindings & decoder
| Recipe       | Does                                                                                                               |
|--------------|--------------------------------------------------------------------------------------------------------------------|
| `py`         | Build the `dif` Python extension (profile `dev-release` = optimized + debug info, so bench timings are realistic). |
| `setup-wasm` | One-time wasm toolchain (`wasm32-wasip1` target, `cargo-zigbuild`, pinned `wasm-bindgen-cli`).                     |
| `wasm`       | Build the browser decoder into `web/pkg` (all 7 codecs cross-compiled via `zig cc`).                               |
| `regen-demo` | Re-emit the committed demo `.dif` for the current format (run `py` first).                                         |

### `.drawio` rendering
| Recipe              | Does                                                                           |
|---------------------|--------------------------------------------------------------------------------|
| `drawio-setup`      | Pull the `rlespinasse/drawio-export` container + `pnpm install` the extension. |
| `drawio-png IN OUT` | Render a `.drawio` to PNG via the local container (no diagrams.net).           |

### Python tools / tests
| Recipe    | Does                                                       |
|-----------|------------------------------------------------------------|
| `pytest`  | Run the pytest suite (run `py` first).                     |
| `lint-py` | `black --check`, `ruff check`, `ty check` (must be clean). |
| `fmt-py`  | `black` + `ruff check --fix`.                              |

### Spec & aggregate
| Recipe | Does                                                   |
|--------|--------------------------------------------------------|
| `spec` | Compile the Typst spec.                                |
| `ci`   | `test-all` + `spec` — what the repo enforces for Rust. |

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

### `bench-codecs` — rank codecs over the `.difr` body
```sh
just bench-codecs testdata/                       # whole tree
just bench-codecs img.png --repeats 5 --strategy arithmetic
just bench-codecs testdata/ --numthreads 4        # adds the rust mt probe
```
Encodes each image to a raw `.difr` body, then compresses that body with **every
registered codec** (deflate, brotli, bzip3, lz4/lz4hc, zstd, lzav, kanzi…) and
ranks them by the metric

> **M = 4·log(S) − log(C)/2 − log(D)** — higher is better,
> where S = original/compressed size, C = memcpy/compress speed, D =
> memcpy/decompress speed (speeds normalised to a memcpy baseline).

Every codec's output is **roundtrip-checked** (`decompress(compress(x)) == x`);
a mismatch marks the row failed instead of crashing the run.

Options: `--strategy` (dark-theme synthesis: `arithmetic`/`invert`/`keep`, default
`arithmetic`) · `--repeats` (timing reps, default 5) · `--numthreads N` (default 1;
**>1** adds rust `dif-{codec}` / `dif-{codec}-mt` rows that drive the real `.dif`
multithreaded encode — zstd `NbWorkers`, brotli `compress_multi` — through the
roundtrip check, the only place that mt path is verified) · `--out`
(`bench-codecs.tsv`) · `--report` (`bench-report.md`).

### `bench-formats` — compare `.dif` against other image formats
```sh
just bench-formats testdata/
just bench-formats testdata/ --numthreads 4       # parallel jxl/avif/brotli + dif -mt rows
just bench-formats img.png --dif-codecs zstd-3 brotli-11
```
Renders each input (including `.drawio` → PNG once, cached) to a common raster,
then encodes it as **PNG, lossless WebP/JXL/AVIF, GIF**, and `.dif` at several
codec variants — reporting size (relative to PNG), encode/decode MB/s, and a
losslessness flag. PNG is the `rel` baseline; GIF and any non-lossless result are
tagged `LOSSY`.

Options: `--repeats` (default 3) · `--numthreads N` (default 1 = a 1-core
comparison; **>1** scales jxl/avif/brotli encode and adds `dif-{codec}-mt` rows)
· `--dif-codecs VARIANT…` (which DIF codec variants to include) · `--out`
(`bench-formats.tsv`) · `--report` (`bench-formats.md`).

> Note: `bench-formats` does **not** roundtrip-check its `dif-*` rows (it reports
> them lossless by construction); use `bench-codecs --numthreads N` to actually
> verify the mt encode path. See [`docs/bench-formats-mt.md`](docs/bench-formats-mt.md)
> for a committed sample report.
