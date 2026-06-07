# Critical rule
- Never use non-ascii character, especially em-dash, en-dash, arrow symbols. They can be replaced with markdown or typst syntax, and simple multi-character symbols.
- Before claiming done, run `just ci` command to test all and check coverage. The Rust test coverage should be 100% on functions and at least 80% on lines (across every `cov-all` feature config, not just `native`). If any python-related code has been modified, run `just py-ci` command to ensure. Python coverage.py has no function metric; instead every Python file must reach at least 80% line coverage on its own (not just the overall total) --- `just py-cov` enforces this per-file floor.
- For each edit phase in Rust, before claiming phase complete, run `just check` and `just clippy` to check if the code pass the basic syntax check.
- For each edit phase in Python, before claiming phase complete, run `just py-test` to ensure every testcase has been passed.

## Repo layout

| Path                                                             | What                                                                                             |
|------------------------------------------------------------------|--------------------------------------------------------------------------------------------------|
| `crates/dif-core/`                                               | Rust core: container (`codec.rs`), raw layout (`format.rs`), dark-theme derivation (`derive.rs`) |
| `crates/dif-py/`                                                 | Python extension (`dif`), built with maturin                                                     |
| `crates/dif-wasm/`                                               | Browser decoder (wasm)                                                                           |
| `crates/lzav-shim/`, `crates/kanzi-shim/`, `crates/libbsc-shim/` | C-codec shims for the benchmark                                                                  |
| `py/dif_tools/`                                                  | Python converters: image/`.drawio` -> `.dif`                                                     |
| `py/bench/`                                                      | Codec + format benchmark harness                                                                 |
| `py/tests/`                                                      | Python test suite                                                                                |
| `web/demo/`                                                      | In-browser viewer (theme-aware)                                                                  |
| `web/extension/`                                                 | VS Codium extension                                                                              |
| `web/wasm-test/`                                                 | node smoke test for the wasm decoder                                                             |
| `docs/`                                                          | Typst spec (`docs/spec/`), design docs, benchmark reports                                        |
| `data/`                                                          | Sample diagrams + photos (`testdata/`), example `.dif`s (git-LFS)                                |
| `third_party/`                                                   | Vendored drawio (submodule)                                                                      |


## justfile recipes

A bare `just` (or `just --list`) prints every recipe.

### Rust core (`dif-core`)
| Recipe              | Does                                                                                 |
|---------------------|--------------------------------------------------------------------------------------|
| `build`             | Build the no_std+alloc default (store/deflate/lz4) --- also the portability check.   |
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
| Recipe                  | Does                                                                                                                    |
|-------------------------|-------------------------------------------------------------------------------------------------------------------------|
| `ext-build`             | Stage the wasip1 decoder (all 8 codecs) + wasi shim into `web/extension/media/`, then compile the TypeScript.           |
| `ext-package`           | Build `dif-viewer.vsix` into `dist/`.                                                                                   |
| `ext-install [variant]` | Package, then install via the editor CLI --- `variant` is the binary on PATH: `code` (default), `codium`, `cursor`, ... |
| `ext-test`              | Typecheck the extension TypeScript (`tsc -p`; skips without node/pnpm).                                                 |

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
| Recipe | Does                                            |
|--------|-------------------------------------------------|
| `spec` | Compile the Typst spec.                         |
| `ci`   | `test-all` + `spec` --- what the repo enforces. |

## Final Information
Look at [`README.md`](README.md) if you cannot find enough information here.
