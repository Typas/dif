# Frequency-Based Palette Index Ordering

Date: 2026-06-01

## Status

- [x] Two-pass frequency-ordered build of `indexed_from_rgba8`
- [x] Deterministic tie-break (ascending packed key)
- [x] `std` `HashMap` / default-build `alloc::BTreeMap` fallback (drops std-only gate)
- [x] Tests: frequency order, tie-break, pixel reconstruction
- [x] Verified: `just test` (16), `just test-native` (23), no_std `just build`, `just pytest` (34)
- [ ] V3: per-frame subpalette (GIF local color table)

## Problem

Indexed encoding currently assigns palette indices by **first-appearance order**.
`indexed_from_rgba8` (`crates/dif-core/src/lib.rs:262`) scans pixels row-major;
each new color gets `id = palette.len()` and is pushed. Index = order of first
occurrence, no regard for how often a color is used.

Per-pixel indices are serialized as **varint** (`crates/dif-core/src/format.rs:98`).
Varint cost:

- index `0..=127` -> 1 byte
- index `128..=16383` -> 2 bytes

With first-appearance order, a frequently-used color can land at a high index and
cost 2 bytes on every pixel. For images with **>128 unique colors** (photos,
gradients) this inflates the pre-zstd body.

## Goal

Smaller output on many-color images. Assign the lowest indices to the most
frequent colors so the hottest pixels emit 1-byte varints. Benefit is real when
the palette exceeds 128 colors; at <=128 colors every index already fits in one
byte (only a marginal entropy effect remains). Decode is unaffected — palette
order carries no semantics.

## Design

### Two-pass build of `indexed_from_rgba8`

Rewrite `indexed_from_rgba8` from one pass to two:

1. **Pass 1 — count.** `HashMap<u32, u32>` keyed by the packed color
   (`u32::from_le_bytes([r, g, b, a])`), value = occurrence count. Scan every
   rgba chunk and tally.
2. **Build palette.** Collect the map entries, sort by **count descending**,
   tie-break by **color-key ascending** (deterministic order -> reproducible
   output bytes, stable tests). Materialize the sorted colors into the
   `Vec<Rgba>` palette. Then **reuse the same HashMap**: overwrite
   `map[color] = final_index`, where `final_index` is the color's rank in sorted
   order. One HashMap for the whole function; no separate remap vector.
3. **Pass 2 — encode.** Scan the rgba buffer again; for each pixel
   `frame.push(map[color])`. Indices are final on emit — **no remap step**.

The packed-`u32` key works for 8-bit rgba (the only input shape of this
function). Memory footprint: one HashMap plus the palette and frame vectors,
same order as today.

### Determinism

Equal-count colors are ordered by ascending packed key. Output is byte-for-byte
reproducible across runs and platforms, so existing snapshot/round-trip tests
stay stable.

### Default-on, no flag

Output bytes change (palette order and index values) but decode is bit-identical
to the source pixels, so there is no compatibility concern — re-encoding an old
file just produces a smaller equivalent. No opt-in flag. Cost: a second scan and
a per-pixel hashmap lookup on encode; accepted in exchange for smaller bodies on
many-color images.

## Scope

In scope:

- `indexed_from_rgba8` (`crates/dif-core/src/lib.rs`) — the only production
  indexed builder; the converter (`dif_tools/convert.py:109`) routes all
  still-image indexed encodes through it.

Out of scope / untouched:

- `Image.indexed()` (Rust binding) — test-only, caller supplies pre-built index
  frames and controls order; no raw colors to count, reordering there would be a
  true remap. Left unchanged.
- Grayscale path, decoder, on-disk format, `validate()`.

## Multi-frame / "across all frames"

The ordering principle is **count over all frames the palette serves**, then
encode. Today the only raw-pixel builder is single-frame, so n = 1 and the
principle collapses to "count this one frame." When a raw multi-frame builder is
added later, pass 1 counts across every frame and pass 2 encodes every frame
against the single shared palette — same two-pass shape.

## Future work (V3)

- **Per-frame subpalette** (GIF-style local color table): each frame carries its
  own smaller palette with indices scoped to that frame. This is a format change
  (encode, decode, `validate`, spec, wasm/js viewer) and is deferred to V3.
  Frequency ordering would then apply within each frame's local table.

## Tests

- **Frequency order:** rgba with known counts (e.g. color A x3, B x1) ->
  `palette[0] == A`, `palette[1] == B`, frame indices match.
- **Tie-break determinism:** two colors with equal count -> ordered by ascending
  packed key.
- **Round-trip:** decode of the re-ordered image equals the original pixels.
- **Regression:** existing `tests/test_codec.py` and Rust unit tests stay green.

## Performance

Pass 1 tally O(pixels) + sort O(k log k) for k colors + pass 2 O(pixels). The
extra image scan and hashmap lookups are negligible next to the zstd codec stage.
