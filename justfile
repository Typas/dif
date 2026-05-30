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
py:
    uv run maturin develop -m crates/dif-py/Cargo.toml

# Build the wasm decoder into web/pkg.
wasm:
    wasm-pack build crates/dif-wasm --target web --out-dir "$PWD/web/pkg"

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
