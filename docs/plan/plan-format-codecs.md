# Plan: wire the 7 chosen codec variants into the `.dif` format

## Goal
`bench formats` should be able to encode/decode `.dif` with each of the 7 codec
variants chosen in `plan.md`, and compare them against the *default* settings of
PNG / WebP-ll / JXL-ll / AVIF-ll / GIF. Today `to_dif` only exposes 5 codec
*families* at hardcoded levels, so the study's picks (e.g. `zstd-3`, `brotli-11`,
`lzav-1`, `lz4-fast1`) cannot be produced.

## Header change: codec byte + level byte
Today the `.dif` header stores **one codec byte** (`CodecId`, see
`crates/dif-core/src/codec.rs`). Add a second **level byte** right after it, so
the container records both *which family* and *which level* produced the body:

```text
old:  magic:"DIF1"  version:u8  codec:u8           raw_len:u64  body
new:  magic:"DIF1"  version:u8  codec:u8  level:u8 raw_len:u64  body
```

Header min size 14 -> **15 bytes**. Rationale (per design discussion): lzbench
lists tens of methods, so a full `u8` codec space (2^8) is wanted; LZ4-fast
acceleration ranges up to ~2^7, so a full `u8` level is wanted. One byte each.

The layout change is breaking, so **bump `VERSION`** (`crates/dif-core/src/format.rs`).
No long-lived persisted `.dif` corpus exists, so no migration path is needed ---
old readers should reject the new version. **One committed asset must be
regenerated**: `web/demo/flowchart.dif` (the demo the wasm viewer loads); add a
step to re-encode it after the format change so `web/demo/` keeps working.

Decode for zstd / brotli / deflate / lz4 does **not** need the level (the
compressed stream is self-describing), so the stored `level` is informational +
forward-compatible; `decompress` ignores it. It exists to make the file fully
self-describing and to let tooling report what produced a given `.dif`.

The 7 named variants map to **(codec byte, level byte)** pairs --- no collapsing:

  | variant        | family          | codec byte | level byte |
  |----------------|-----------------|-----------:|-----------:|
  | `zstd-3`       | Zstd            | 4 (exists) |          3 |
  | `zstd-10`      | Zstd            | 4 (exists) |         10 |
  | `brotli-5`     | Brotli          | 2 (exists) |          5 |
  | `brotli-11`    | Brotli          | 2 (exists) |         11 |
  | `libdeflate-6` | Deflate         | 1 (exists) |          6 |
  | `lz4-fast1`    | Lz4  (**new**)  | 5 (**new**)|          1 |
  | `lzav-1`       | Lzav (**new**)  | 6 (**new**)|          1 |

So the format change: **add a header level byte**, **add 2 codec IDs (Lz4,
Lzav)**, **thread a level parameter through the encode path**. Decode gains
lz4/lzav readers only.

`libdeflate-6` reuses the existing `Deflate` family (standard DEFLATE stream):
encode with the `libdeflater` crate under `native`, decode with `miniz_oxide`
everywhere (interoperable). No new codec byte, level byte = 6.

## Scope

### 1. `dif-core` --- codec.rs
- Add `CodecId::Lz4 = 5`, `CodecId::Lzav = 6`; extend `from_u8`.
- Add the `level:u8` to the header: in `to_dif`, `out.push(level)` after the
  codec byte; bump the `Vec::with_capacity(... + 15)` and the `raw_len` write to
  the new offset. In `from_dif`, read `codec = bytes[5]`, `level = bytes[6]`,
  `raw_len = bytes[7..15]`, body = `bytes[15..]`; update the `len < 15` guard.
- Change `compress(codec, data)` -> `compress(data, codec, level: u8)` with a
  documented per-family meaning. `to_dif` gains a `level: u8` argument;
  `decompress` keeps its `(codec, data, raw_len)` shape --- **the level byte is
  read from the header but not passed to decode** (level-agnostic).
- Per-family level wiring:
  - Deflate: `miniz_oxide::deflate::compress_to_vec(data, level)`; under
    `native`, prefer `libdeflater` encoder at `level` (still standard DEFLATE).
  - Brotli: `CompressorWriter::new(out, 4096, quality=level, 22)`.
  - Zstd: `zstd_safe::compress(out, data, level)`.
  - Xz: `preset = level`.
  - Lz4: `lz4_flex` (pure-Rust, no_std + alloc -> **wasm-decodable**). Encode at
    the requested acceleration; decode with known `raw_len`.
  - Lzav: native-only FFI (see Risks). Decode needs `raw_len` (have it).
- Default level per family kept sensible so the no-arg path is unchanged.

### 2. `dif-core` --- Cargo + features
- Add deps: `lz4_flex` (default, no_std), `libdeflater` + `lzav` C shim under
  `native`.
- Lz4 stays in the portable (no_std) set so it builds everywhere. Lzav and the
  libdeflate *encoder* are C-linked -> gated. Whether they reach the **wasm
  decoder** depends on the wasm build strategy below.

### 2b. Wasm codec strategy (C deps in the browser decoder)
The decoder should ideally read every codec it can write. Lzav (and, if we want
in-browser zstd/brotli) are C-linked, which `wasm-pack`'s plain
`wasm32-unknown-unknown` build can't compile. Two options:

- **Option 1 --- cross-compile C->wasm with Zig (preferred).** Use
  `cargo-zigbuild`, which wires `zig cc` in as the C compiler so the `cc`-crate
  deps (lzav shim, optionally zstd-safe) compile to wasm. Browser target stays
  `wasm32-unknown-unknown` + `wasm-bindgen` (so the existing bindgen flow holds);
  replace the `wasm-pack build` step with `cargo zigbuild --target
  wasm32-unknown-unknown` then a `wasm-bindgen` pass. Keeps **one Rust codebase**
  and reuses `dif-core`'s decode path verbatim --- the wasm decoder then supports
  Store/Deflate/Xz/Lz4 **+ Lzav (+ Zstd/Brotli if enabled)**.
  - Gotcha: C-ABI on `wasm32-unknown-unknown` is partial --- needs
    `-Zwasm-c-abi=spec` (recently in Rust). Verify on the pinned toolchain;
    `wasm32-wasip1` is the non-browser fallback.
  - Cost: adds `zig` + `cargo-zigbuild` to the build toolchain (`drawio-setup`/
    a `wasm-setup` recipe), and a Rust feature (e.g. `wasm-native`) that turns on
    the C decoders for the wasm crate.
- **Option 2 --- C-only wasm decoder (whole decoder in C).** Reimplement the
  *entire* `.dif` decode path --- container parse, all codec decoders, theme
  render --- in C, compiled straight to wasm via `zig cc`/clang. Every C codec lib
  (lzav, zstd, ...) then links natively to wasm with no Rust C-ABI gotcha, and
  `dif-wasm`/`wasm-pack` is dropped. Cost: a **second full decoder
  implementation** that must stay byte-for-byte in sync with the Rust
  `dif-core` format/codec logic --- two sources of truth for the container, double
  the test surface. Only worth it if Option 1's `-Zwasm-c-abi=spec` path proves
  unworkable on the pinned toolchain.

**Decision needed before phase 1 touches `dif-wasm`/`extension`.** Default to
Option 1. If neither lands in time, Lzav `.dif` simply won't decode in-browser
(same degraded-codec pattern as Zstd/Brotli today) --- encode/bench still work via
the native Python path.

### 3. `dif-py` --- lib.rs + stub
- `codec_id` accepts the 7 **variant strings** (`"zstd-3"`, `"brotli-11"`,
  `"lzav-1"`, `"lz4-fast1"`, `"libdeflate-6"`, ...) and maps each to
  `(CodecId, level)`. Keep the bare family names (`"zstd"`, `"brotli"`, ...) as
  aliases for their study-chosen default level.
- `to_dif(codec="zstd-3")` --- single string arg, no separate `level` kwarg, so
  the Python surface stays one knob. Update `typings/dif.pyi` docstring/Literal.

### 4. `bench/compare.py`
- Replace the single hardcoded `img.to_dif("brotli")` row with a loop over the 7
  variants, emitting rows `dif-zstd-3`, `dif-brotli-11`, ... . Other formats stay
  one default row each.
- Add `--outer-codecs` to the `formats` subparser (default = all 7) so a run can
  narrow the set. Relative-size column (`rel`) keys off a chosen baseline (e.g.
  `dif-zstd-3`).

### 5. spec --- `docs/spec/dif-spec.typ`
- Document the new 15-byte header with the `level:u8` field after `codec:u8`.
- Document codec bytes 5 (Lz4) and 6 (Lzav); state that `level` is recorded for
  provenance/forward-compat but is **not consumed by decode**.

### 6. Tests
- `dif-core`: extend `dif_roundtrip_all_codecs` to include Lz4 (portable) and
  Lzav (native); add a level-roundtrip case (encode `zstd-3` vs `zstd-10`, both
  decode equal).
- `tests/test_convert.py`: roundtrip each of the 7 variant strings through
  `to_dif`/`from_dif`.

## Risks / unknowns
- **Lzav in Rust**: no vetted pure-Rust crate assumed. Plan = vendor `lzav.h`
  and compile via a `build.rs` + `cc` shim, mirroring `crates/kanzi-shim`
  (the bench already builds an lzav C shim in `bench/native.py`). Native-only;
  unavailable in the wasm decoder. **Confirm crate landscape before coding.**
- **libdeflate**: `libdeflater` is C-linked -> `native` only. Decode path stays
  on `miniz_oxide`, so wasm still reads `libdeflate-6` files.
- **wasm decoder**: baseline gains Store/Deflate/Xz/**Lz4**. Lzav (and
  Zstd/Brotli) in-browser depend on the Sec.2b Zig cross-compile landing; if it
  doesn't, they stay native-only (pre-existing degraded pattern).
- **Level semantics differ per family** (brotli 0--11, zstd 1--22, lz4
  acceleration, deflate 0--9). The variant-string map in `dif-py` is the single
  source of truth; `dif-core` just forwards an integer.

## Out of scope
- Adding lz4hc / kanzi / bzip3 to the format (eliminated in the study).
- Changing the header layout beyond the new codec byte values.
- The `--report`/`speed()` fixes in `bench/compare.py` (already applied this
  session).

## Suggested commit slicing
1. `feat(dif): parameterize codec level, add lz4 + lzav codec ids` (dif-core +
   Cargo + spec + core tests).
2. `feat(dif-py): accept the 7 codec variant strings` (lib.rs + stub +
   test_convert).
3. `feat(bench): compare DIF across all 7 codec variants` (compare.py +
   `--outer-codecs`).
