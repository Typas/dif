# DIF project automation. Run from the `final/` project root.
# Python tasks go through `uv` (project pinned to 3.12, which also sidesteps the
# system-Python PyO3 version cap). maturin/wasm-pack/typst are external tools.

default:
    @just --list

# --- Rust core (dif-core) -------------------------------------------------

# Build the no_std+alloc default (store/deflate/xz); clean build = portability check.
build:
    cargo build -p dif-core

build-std:
    cargo build -p dif-core --features std

build-native:
    cargo build -p dif-core --features native

# Assert the portable core stays no_std (alias for the default build).
check-nostd: build

# Test the core. Bare run = std under cfg(test): store / deflate / xz.
test:
    cargo test -p dif-core

# `native` pulls in brotli + zstd and the liblzma XZ path.
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

# Build the wasm decoder into web/pkg.
wasm:
    wasm-pack build crates/dif-wasm --target web --out-dir "$PWD/web/pkg"

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
