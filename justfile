# DIF project automation. Run from the `dif/` project root.
# Python tasks go through `uv` (project pinned to 3.12, which also sidesteps the
# system-Python PyO3 version cap). maturin/wasm-pack/typst are external tools.
#
# Recipe naming rule: `<component>-<action>[-<variant>]`. Component first.
# dif-core is the default component, so its actions are bare verbs (build,
# test, fmt, clippy). Every other component keeps its prefix (wasm-, ext-,
# drawio-, bench-, py-, spec). Cross-component aggregates are bare verbs
# (default, test-all, ci, clean, regen-demo).

# Python packages live under py/; put it on sys.path for `python -m ...` recipes
# (pytest gets it via tool.pytest pythonpath).
export PYTHONPATH := justfile_directory() / "py"

default:
    @just --list

# --- Rust core (dif-core) -------------------------------------------------

# Build the no_std+alloc default (store/deflate/lz4); clean build = portability check.
# `profile`: dev (default) | release | dev-release. `dev-release` = optimized +
# debug info, the profile dif-py compiles dif-core under. e.g. `just build release`.
build profile="dev":
    cargo build -p dif-core --profile {{profile}}

build-std profile="dev":
    cargo build -p dif-core --features std --profile {{profile}}

build-native profile="dev":
    cargo build -p dif-core --features native --profile {{profile}}

# Assert the portable core stays no_std (alias for the default build).
check-nostd: build

# Test the core. Bare run = std under cfg(test): store / deflate / lz4.
test:
    cargo test -p dif-core

# Test the core with std feature. Bare run = std under cfg(test): store / deflate / lz4.
test-std:
    cargo test -p dif-core --features std

# Test the core with encode feature (OKLab quantize + dark-theme derivation).
test-encode:
    cargo test -p dif-core --features encode

# `native` pulls in brotli + zstd + the libdeflate encoder + the lzav C shim.
test-native:
    cargo test -p dif-core --features native

# dif-core line coverage 
cov: 
    cargo llvm-cov -p dif-core

# dif-core line coverage with std coverage
cov-std: 
    cargo llvm-cov -p dif-core --features std

# dif-core line coverage (cargo-llvm-cov, native feature set so every codec is
# exercised). First run auto-adds the llvm-tools-preview component.
cov-native:
    cargo llvm-cov -p dif-core --features native

# Same as `cov`, but names the exact uncovered lines per file (for chasing gaps).
cov-native-missing:
    cargo llvm-cov -p dif-core --features native --show-missing-lines

# Core matrix (all tiers build, both test sets pass) + the Python, wasm, and
# extension suites.
#
# Every feature tested: core matrix + py-test + wasm-test + ext-test.
test-all: build build-std build-native test test-std test-encode test-native py-test wasm-test ext-test

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --all --all-features -- -D warnings

check:
    cargo check --all

# --- Bindings (excluded from the cargo workspace) -------------------------

# Build the Python extension into the uv 3.12 env.
# `dev-release` = optimized + debug info, so benchmark timings are realistic.
# patchelf (a uv dev dep) lets maturin's rpath patch succeed and silences the
# rpath warning. The `rm -rf target/maturin` fixes reruns: on a cached rebuild
# maturin re-populates its staging copy over the prior one and collapses the
# cdylib's hardlinks to a 0-byte inode, so the next parse fails with "Object is
# too small". Wiping the staging dir first gives maturin a clean slate.
py:
    rm -rf target/maturin
    uv run maturin develop --profile dev-release -m crates/dif-py/Cargo.toml

# Target is wasm32-wasip1 so wasi-libc provides malloc + the C headers the codec
# deps need (zstd/lzav). `cargo-zigbuild` + `ziglang` (the zig binary wheel) come
# from the uv dev env; `wasm-bindgen-cli` is version-pinned to the wasm-bindgen
# crate so the JS glue matches the .wasm.
# One-time wasm toolchain (wasm32-wasip1 target, cargo-zigbuild, pinned wasm-bindgen-cli).
wasm-setup:
    rustup target add wasm32-wasip1
    uv sync
    cargo install wasm-bindgen-cli --version 0.2.122 --locked

# The C codecs (zstd id 4, lzav id 6) cross-compile through `zig cc`
# (cargo-zigbuild) against wasi-libc, so the browser decodes all 8 variants —
# including the default zstd-3 — not just the pure-Rust store/deflate/brotli/lz4
# set. wasm-bindgen then emits the JS glue. The wasip1 module needs a small wasi
# shim in the loader (see web/demo/main.js). Run `just wasm-setup` once first.
# Build the wasm decoder into dist/pkg (all 8 codecs, cross-compiled via zig cc).
wasm:
    uv run cargo-zigbuild build --release --target wasm32-wasip1 \
        --manifest-path crates/dif-wasm/Cargo.toml
    wasm-bindgen target/wasm32-wasip1/release/dif_wasm.wasm \
        --target web --out-dir "$PWD/dist/pkg"

# Load dist/pkg + the wasi shim and decode web/demo/flowchart.dif. Node has no
# import maps, so a resolve hook wires the glue's bare `wasi_snapshot_preview1`
# import to web/demo/wasi_shim.js. Run `just wasm` first to build dist/pkg.
#
# Smoke-test the wasm decoder in node (skips without node or dist/pkg).
wasm-test:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v node >/dev/null || { echo "skip wasm-test: no node"; exit 0; }
    [ -f dist/pkg/dif_wasm.js ] || { echo "skip wasm-test: dist/pkg missing (run just wasm)"; exit 0; }
    node --import "{{justfile_directory()}}/web/wasm-test/wasi-resolve.mjs" \
        "{{justfile_directory()}}/web/wasm-test/smoke.mjs"

# Re-emit the committed demo asset for the current .dif format (run `just py`
# first so the `dif` module exists). Needed after a container format bump.
regen-demo:
    uv run python py/regen_flowchart.py

# Convert one image (PNG/TIFF/JPEG/...) or a .drawio diagram to .dif using the
# shipped triplet: outer=store (seekable random-access), palette=zstd-16,
# frame=zstd-10, with a 2-theme palette (light source + OKLab-derived dark).
# Run `just py` first so the `dif` module exists. Override any arg, e.g.
# `just convert in.png out.dif strategy=keep` for a single (light) theme, or
# `just convert in.png out.dif frame_codec=brotli-11`.
convert IN OUT threads="1" index_width="auto" frame_codec="zstd-10" palette_codec="zstd-16" outer_codec="store" strategy="arithmetic":
    uv run python -m dif_tools convert "{{IN}}" "{{OUT}}" \
        --theme-strategy {{strategy}} \
        --codec {{outer_codec}} \
        --palette-codec {{palette_codec}} \
        --frame-codec {{frame_codec}} \
        --index-width {{index_width}} \
        --threads {{threads}}

# Regenerate every committed .dif under data/dif-examples/ from its
# data/testdata/ source via `just convert` (the shipped triplet + 2-theme
# palette). .drawio inputs reuse the cached PNG render under out/drawio-png/
# (run `just drawio-setup` once if a render is needed). Run `just py` first.
# The .dif examples are LFS-tracked; `git add` re-cleans them to pointers.
regen-examples:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    for src in data/testdata/drawio/*.drawio; do
        just convert "$src" "data/dif-examples/drawio/$(basename "${src%.drawio}").dif"
    done
    for src in data/testdata/usc-sipi-misc/*.tiff; do
        just convert "$src" "data/dif-examples/usc-sipi-misc/$(basename "${src%.tiff}").dif"
    done
    n=(data/dif-examples/*/*.dif); echo "regenerated ${#n[@]} examples"

# --- VSCodium / VS Code extension -----------------------------------------
# The extension reuses the wasip1 decoder built by `just wasm` (all 8 codecs)
# plus its wasi shim, so the custom editor decodes the same files the browser
# demo does — including the default zstd-3 `.dif`. `build:wasm` (wasm-pack) is
# gone: dif-wasm pulls the C codecs, which only build through the zig/wasip1 path.

# Build the extension: stage the wasm decoder + shim into media/, then compile TS.
ext-build: wasm
    rm -rf web/extension/media/pkg
    cp -r dist/pkg web/extension/media/pkg
    cp web/demo/wasi_shim.js web/extension/media/wasi_shim.js
    pnpm --dir web/extension install
    pnpm --dir web/extension run compile

# Package a .vsix into dist/. Install via the editor GUI (Extensions >
# Install from VSIX) in VS Code / VSCodium / Cursor / any VS Code-family editor,
# or use `just ext-install` below.
ext-package: ext-build
    mkdir -p "{{justfile_directory()}}/dist"
    cd web/extension && pnpm dlx @vscode/vsce package --no-dependencies --out "{{justfile_directory()}}/dist/dif-viewer.vsix"

# Install the packaged extension via an editor CLI. `variant` is the editor
# binary on PATH: `code` (default), `codium`, `cursor`, ...
ext-install variant="code": ext-package
    {{variant}} --install-extension "{{justfile_directory()}}/dist/dif-viewer.vsix"

# Standalone tsc -p: no wasm build needed at compile time.
#
# Typecheck the extension TypeScript (skips without node/pnpm).
ext-test:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v node >/dev/null && command -v pnpm >/dev/null || { echo "skip ext-test: no node/pnpm"; exit 0; }
    pnpm --dir web/extension install --frozen-lockfile || pnpm --dir web/extension install
    pnpm --dir web/extension run compile

# --- drawio rendering (local container) ------------------------------------
# rlespinasse/drawio-export bundles drawio-desktop + a headless browser (xvfb),
# run one-shot per file. Fully local, no diagrams.net. dif_tools.drawio drives
# it (copies the diagram into a scratch dir, reads /data/out/diagram.png back).

drawio_image := "docker.io/rlespinasse/drawio-export:v4.52.0"

# Pull the render image. Podman + Python only; extension/pnpm deps live in `just ext-build`.
drawio-setup:
    podman pull {{drawio_image}}

# Render a .drawio to PNG via the local container. SCALE defaults to 2; lower it
# (e.g. `just drawio-png in.drawio out.png 1`) for wide diagrams on low-RAM hosts
# -- render memory grows with scale squared.
drawio-png IN OUT SCALE='2':
    uv run python -c "from dif_tools.drawio import render_drawio_to_png as r; print(r('{{IN}}', '{{OUT}}', {{SCALE}}))"

# --- Python tools / tests -------------------------------------------------

# Build optional native benchmark codecs (lzav + kanzi + libbsc shims). Wipe the
# staged shims first: when wrapper.cpp's exported symbols change (e.g. a new
# libbsc entry point), the .so already on disk is ABI-stale and the ctypes symbol
# wiring would crash at import -- before the rebuild ever runs. Same reasoning as
# `just py` wiping target/maturin. A missing .so is handled gracefully (rebuilt).
# Pass `--cuda` to build libbsc's GPU sort transforms (-m7/-m8): `just bench-setup
# --cuda`. That build needs nvcc on PATH, OpenMP installed system-wide (<omp.h> +
# libgomp/libomp, e.g. `apt install libomp-dev`), and an NVIDIA GPU at run time.
# Default is CPU-only (no nvcc/OpenMP needed).
bench-setup *ARGS:
    rm -rf py/bench/_native
    uv run python -m bench setup {{ARGS}}

# Rank codecs over a .difr body by the M metric.
bench-codecs *ARGS:
    uv run python -m bench codecs {{ARGS}}

# Compare DIF against PNG / WebP / JXL / AVIF / GIF.
bench-formats *ARGS:
    uv run python -m bench formats {{ARGS}}

# Python test suite (run `just py` first so the `dif` module exists).
py-test:
    uv run pytest

# Python line+branch coverage over dif_tools + bench (coverage.py via pytest-cov;
# source configured in pyproject). Run `just py` first.
py-cov:
    uv run pytest --cov --cov-report=term-missing

# Repo requires these clean.
py-lint:
    uv run black --check .
    uv run ruff check .
    uv run ty check

py-fmt:
    uv run black .
    uv run ruff check --fix .

# --- Spec -----------------------------------------------------------------

# Compile the spec; repo convention requires it to build.
spec:
    typst compile docs/spec/dif-spec.typ

# --- Aggregate ------------------------------------------------------------

# Rust feature matrix + spec compile (what the repo actually enforces for Rust).
# fmt-check/clippy are opt-in: the repo doesn't keep dif-core rustfmt-clean.
ci: test-all spec
    @echo "core + spec OK"

# --- Cleanup --------------------------------------------------------------

# `dif` is uninstalled from the uv project env (`.venv`) only — `uv pip` never
# touches system pip. Leaves tracked sources (web/extension/media/viewer.js) and
# deps (node_modules).
# Remove build artifacts: cargo target, staged wasm + TS output, dist, py caches,
# bench native shims, dif module. Keeps all benchmark records (bench-*.md/.tsv).
clean:
    cargo clean
    -uv pip uninstall dif
    rm -rf web/extension/media/pkg web/extension/out .pytest_cache dist
    rm -rf py/dif_tools/__pycache__ py/bench/__pycache__ py/tests/__pycache__
    rm -rf py/bench/_native
    rm -f web/extension/media/wasi_shim.js *.vsix
