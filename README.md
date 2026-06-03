# DIF — Modern Diagram Image Format (`.dif`)

A lossless, palette-based image format built for **diagrams** and **theme-aware
display**. Like GIF it maps pixels through a colour palette, but each colour
carries one entry *per named theme* (e.g. `light` / `dark`), so a single file
re-themes itself to match the browser or editor instead of staying a fixed
bitmap. Grayscale (8/16-bit) and APNG/GIF-style frames are supported, and the
compressed `.dif` body uses one of 8 study-chosen lossless codecs.

- Format spec: [`docs/spec/dif-spec.typ`](docs/spec/dif-spec.typ)
- Design + worklog: [`docs/plan.md`](docs/plan.md), [`docs/plan-format-codecs.md`](docs/plan-format-codecs.md)

## Repo layout

| Path                                      | What                                                                                             |
|-------------------------------------------|--------------------------------------------------------------------------------------------------|
| `crates/dif-core/`                        | Rust core: container (`codec.rs`), raw layout (`format.rs`), dark-theme derivation (`derive.rs`) |
| `crates/dif-py/`                          | Python extension (`dif`), built with maturin                                                     |
| `crates/dif-wasm/`                        | Browser decoder (wasm)                                                                           |
| `crates/lzav-shim/`, `crates/kanzi-shim/` | C-codec shims for the benchmark                                                                  |
| `py/dif_tools/`                           | Python converters: image/`.drawio` → `.dif`                                                      |
| `py/bench/`                               | Codec + format benchmark harness                                                                 |
| `py/tests/`                               | Python test suite                                                                                |
| `web/demo/`                               | In-browser viewer (theme-aware)                                                                  |
| `web/extension/`                          | VS Codium extension                                                                              |
| `web/wasm-test/`                          | node smoke test for the wasm decoder                                                             |
| `docs/`                                   | Typst spec (`docs/spec/`), design docs, benchmark reports                                        |
| `data/`                                   | Sample diagrams + photos (`testdata/`), example `.dif`s (git-LFS)                                |
| `third_party/`                            | Vendored drawio (submodule)                                                                      |

## Prerequisites

- [`uv`](https://docs.astral.sh/uv/) — all Python tasks (project pins 3.12).
- Rust toolchain (`cargo`) — the core and bindings.
- [`just`](https://github.com/casey/just) — task runner (recipes below).
- `git-lfs` — `data/testdata/` images and `*.tsv` reports are LFS-tracked.
- Optional: `podman` + `pnpm` (`.drawio` rendering / extension), `typst` (spec),
  a C compiler (`cc`) and network (benchmark `lzav`/`kanzi` shims).

## Quick Start

```sh
# 1. Build the Rust core and run its tests (portable no_std tier).
just test

# 2. Build the Python extension `dif` into the uv env (needed by the
#    converter, the benchmarks, and the tests).
just py

# 3. Convert an image (or a .drawio) to a themed .dif.
#    (py/ holds the Python packages, so put it on the path.)
PYTHONPATH=py uv run python -m dif_tools convert data/testdata/usc-sipi-misc/4.1.01.tiff out.dif
#   --codec zstd-3|zstd-10|brotli-5|brotli-11|lz4-fast1|lzav-1|libdeflate-6|store
#   --theme-strategy arithmetic|invert|keep   (how the dark theme is derived)
#   --raw                                     (write uncompressed .difr instead)

# 4. Run the Python test suite.
just py-test

# 5. (Optional) View in the browser, theme-aware.
just wasm-setup   # one-time toolchain
just wasm         # build the decoder into dist/pkg
#   then serve the repo root and open web/demo/ (the page loads ../../dist/pkg);
#   it picks light/dark from the browser.
```

A bare `just` (or `just --list`) prints every recipe.

> [!NOTE]
> `just wasm-setup` prints deprecation warnings for `multipart` and
> `buf_redux` while building `wasm-bindgen-cli` from source. These are upstream
> transitive dependencies (`buf_redux` <- `multipart` <- `rouille` <-
> `wasm-bindgen-cli`); `rouille` only backs the `wasm-bindgen-test-runner`
> binary, which this project never runs. The warnings are harmless and not
> fixable from this repo: `0.2.122` is already the latest `wasm-bindgen-cli`,
> and the version is pinned to match the `wasm-bindgen` crate. To silence them,
> install a prebuilt binary instead of compiling (e.g. `cargo binstall
> wasm-bindgen-cli --version 0.2.122`).

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
| `cov`               | dif-core line coverage (cargo-llvm-cov, native features).                            |
| `test-all`          | Every feature tested: core matrix + `py-test` + `wasm-test` + `ext-test`.            |
| `fmt` / `fmt-check` | `cargo fmt` (write / check).                                                         |
| `clippy`            | Clippy with `--all-features -D warnings`.                                            |

### Bindings & decoder
| Recipe       | Does                                                                                                               |
|--------------|--------------------------------------------------------------------------------------------------------------------|
| `py`         | Build the `dif` Python extension (profile `dev-release` = optimized + debug info, so bench timings are realistic). |
| `wasm-setup` | One-time wasm toolchain (`wasm32-wasip1` target, `cargo-zigbuild`, pinned `wasm-bindgen-cli`).                     |
| `wasm`       | Build the browser decoder into `dist/pkg` (all 8 codecs cross-compiled via `zig cc`).                              |
| `wasm-test`  | Smoke-test the decoder in node: decode `web/demo/flowchart.dif` (run `wasm` first; skips without node).            |
| `regen-demo` | Re-emit the committed demo `.dif` for the current format (run `py` first).                                         |

### VS Code / Codium / Cursor extension
| Recipe                  | Does                                                                                                                |
|-------------------------|---------------------------------------------------------------------------------------------------------------------|
| `ext-build`             | Stage the wasip1 decoder (all 8 codecs) + wasi shim into `web/extension/media/`, then compile the TypeScript.       |
| `ext-package`           | Build `dif-viewer.vsix` into `dist/`.                                                                               |
| `ext-install [variant]` | Package, then install via the editor CLI — `variant` is the binary on PATH: `code` (default), `codium`, `cursor`, … |
| `ext-test`              | Typecheck the extension TypeScript (`tsc -p`; skips without node/pnpm).                                             |

The `.vsix` also installs through the GUI (Extensions ▸ **Install from VSIX…**) in
any VS Code-family editor.

### `.drawio` rendering
| Recipe              | Does                                                                           |
|---------------------|--------------------------------------------------------------------------------|
| `drawio-setup`      | Pull the `rlespinasse/drawio-export` container + `pnpm install` the extension. |
| `drawio-png IN OUT` | Render a `.drawio` to PNG via the local container (no diagrams.net).           |

### Python tools / tests
| Recipe    | Does                                                       |
|-----------|------------------------------------------------------------|
| `py-test` | Run the pytest suite (run `py` first).                     |
| `py-cov`  | pytest suite with coverage.py (dif_tools + bench).         |
| `py-lint` | `black --check`, `ruff check`, `ty check` (must be clean). |
| `py-fmt`  | `black` + `ruff check --fix`.                              |

### Spec & aggregate
| Recipe | Does                                          |
|--------|-----------------------------------------------|
| `spec` | Compile the Typst spec.                       |
| `ci`   | `test-all` + `spec` — what the repo enforces. |

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
just bench-codecs data/testdata/                  # whole tree
just bench-codecs img.png --repeats 5 --strategy arithmetic
just bench-codecs data/testdata/ --numthreads 4   # adds the rust mt probe
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
just bench-formats data/testdata/
just bench-formats data/testdata/ --numthreads 4  # parallel jxl/avif/brotli + dif -mt rows
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

> [!NOTE]
> `bench-formats` does **not** roundtrip-check its `dif-*` rows (it reports
> them lossless by construction); use `bench-codecs --numthreads N` to actually
> verify the mt encode path. See [`docs/bench-formats-mt.md`](docs/bench-formats-mt.md)
> for a committed sample report.
