# DIF project automation. Run from the `final/` project root.
# Python tasks go through `uv` (project pinned to 3.12, which also sidesteps the
# system-Python PyO3 version cap). maturin/wasm-pack/typst are external tools.

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

# `native` pulls in brotli + zstd + the libdeflate encoder + the lzav C shim.
test-native:
    cargo test -p dif-core --features native

# Full core matrix: every feature tier builds, both test sets pass.
test-all: build build-std build-native test test-native

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy -p dif-core --all-features -- -D warnings

# --- Bindings (excluded from the cargo workspace) -------------------------

# Build the Python extension into the uv 3.12 env.
# `dev-release` = optimized + debug info, so benchmark timings are realistic.
py:
    uv run maturin develop --profile dev-release -m crates/dif-py/Cargo.toml

# One-time wasm toolchain. Target is wasm32-wasip1 so wasi-libc provides malloc +
# the C headers the codec deps need (zstd/lzav). `cargo-zigbuild` + `ziglang`
# (the zig binary wheel) come from the uv dev env; `wasm-bindgen-cli` is
# version-pinned to the wasm-bindgen crate so the JS glue matches the .wasm.
setup-wasm:
    rustup target add wasm32-wasip1
    uv sync
    cargo install wasm-bindgen-cli --version 0.2.122 --locked

# Build the wasm decoder into web/pkg. The C codecs (zstd id 4, lzav id 6)
# cross-compile through `zig cc` (cargo-zigbuild) against wasi-libc, so the
# browser decodes all 8 variants — including the default zstd-3 — not just the
# pure-Rust store/deflate/brotli/lz4 set. wasm-bindgen then emits the JS glue.
# The wasip1 module needs a small wasi shim in the loader (see web/main.js).
# Run `just setup-wasm` once first.
wasm:
    uv run cargo-zigbuild build --release --target wasm32-wasip1 \
        --manifest-path crates/dif-wasm/Cargo.toml
    wasm-bindgen crates/dif-wasm/target/wasm32-wasip1/release/dif_wasm.wasm \
        --target web --out-dir "$PWD/web/pkg"

# Re-emit the committed demo asset for the current .dif format (run `just py`
# first so the `dif` module exists). Needed after a container format bump.
regen-demo:
    uv run python web/regen_flowchart.py

# --- VSCodium / VS Code extension -----------------------------------------
# The extension reuses the wasip1 decoder built by `just wasm` (all 8 codecs)
# plus its wasi shim, so the custom editor decodes the same files the browser
# demo does — including the default zstd-3 `.dif`. `build:wasm` (wasm-pack) is
# gone: dif-wasm pulls the C codecs, which only build through the zig/wasip1 path.

# Build the extension: stage the wasm decoder + shim into media/, then compile TS.
ext-build: wasm
    rm -rf extension/media/pkg
    cp -r web/pkg extension/media/pkg
    cp web/wasi_shim.js extension/media/wasi_shim.js
    pnpm --dir extension install
    pnpm --dir extension run compile

# Package a .vsix into the repo root. Install via the editor GUI (Extensions >
# Install from VSIX) in VS Code / VSCodium / Cursor / any VS Code-family editor,
# or use `just ext-install` below.
ext-package: ext-build
    cd extension && pnpm dlx @vscode/vsce package --no-dependencies --out "{{justfile_directory()}}/dif-viewer.vsix"

# Install the packaged extension via an editor CLI. `variant` is the editor
# binary on PATH: `code` (default), `codium`, `cursor`, ...
ext-install variant="code": ext-package
    {{variant}} --install-extension "{{justfile_directory()}}/dif-viewer.vsix"

# --- drawio rendering (local container) ------------------------------------
# rlespinasse/drawio-export bundles drawio-desktop + a headless browser (xvfb),
# run one-shot per file. Fully local, no diagrams.net. dif_tools.drawio drives
# it (copies the diagram into a scratch dir, reads /data/out/diagram.png back).

drawio_image := "docker.io/rlespinasse/drawio-export:v4.52.0"

# Pull the render image (and install the extension deps via pnpm).
drawio-setup:
    podman pull {{drawio_image}}
    pnpm --dir extension install

# Render a .drawio to PNG via the local container.
drawio-png IN OUT:
    uv run python -c "from dif_tools.drawio import render_drawio_to_png as r; print(r('{{IN}}', '{{OUT}}'))"

# --- Python tools / tests -------------------------------------------------

# Build optional native benchmark codecs (lzav + kanzi shim).
bench-setup:
    uv run python -m bench setup

# Rank codecs over a .difr body by the M metric.
bench-codecs *ARGS:
    uv run python -m bench codecs {{ARGS}}

# Compare DIF against PNG / WebP / JXL / AVIF / GIF.
bench-formats *ARGS:
    uv run python -m bench formats {{ARGS}}

# pytest suite (run `just py` first so the `dif` module exists).
pytest:
    uv run pytest

# Repo requires these clean.
lint-py:
    uv run black --check .
    uv run ruff check .
    uv run ty check

fmt-py:
    uv run black .
    uv run ruff check --fix .

# --- Spec -----------------------------------------------------------------

# Compile the spec; repo convention requires it to build.
spec:
    typst compile spec/dif-spec.typ

# --- Aggregate ------------------------------------------------------------

# Rust feature matrix + spec compile (what the repo actually enforces for Rust).
# fmt-check/clippy are opt-in: the repo doesn't keep dif-core rustfmt-clean.
ci: test-all spec
    @echo "core + spec OK"

# --- Cleanup --------------------------------------------------------------

# `dif` is uninstalled from the uv project env (`.venv`) only — `uv pip` never
# touches system pip. Leaves tracked sources (extension/media/viewer.js) and
# deps (node_modules).
# Remove build artifacts: cargo target, staged wasm + TS output, vsix, py caches, dif module.
clean:
    cargo clean
    -uv pip uninstall dif
    rm -rf web/pkg extension/media/pkg extension/out .pytest_cache
    rm -rf dif_tools/__pycache__ bench/__pycache__ tests/__pycache__
    rm -f extension/media/wasi_shim.js dif-viewer.vsix
