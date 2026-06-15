# Region-aware dark-theme derivation

## Context

The current dark-theme derivation (`crates/dif-core/src/derive.rs::arithmetic_rgb_region`)
runs **per palette color, spatially blind**. Each unique source color maps to one index
(`indexed_from_rgba8`) and so to one dark color. Consequences:

1. **Anti-aliasing not detected.** The only AA mechanism is a per-color chroma test
   (`chroma < 1e-3` -> treat as gray, `L' = 1 - L`). A single threshold cannot find a
   grayscale<->color boundary, and an AA pixel is indistinguishable from a gradient pixel
   to a per-color test.
2. **No foreground/background distinction.** Background-like fills and foreground-like
   colored text get the same transform.
3. **One color -> one index -> one dark output.** The same source color in a fill vs a
   text stroke cannot get two different dark colors.

Fix: move derivation from palette-space to **image-space, region-aware**. Classify each
pixel with cheap local spatial operators (filters, no FFT), let one source color split
into multiple indices keyed by region, apply a per-region transform, blend AA pixels
between their two dark endpoints, and run a structural relaxation so the dark render's
edge map matches the light render's.

The container format and wasm decoder are untouched: `palettes[theme][index]` already
allows two indices to share a light color and differ in dark color. All new code is
`#[cfg(feature = "encode")]`; decode just reads a (possibly longer) palette. The light
theme stays **bit-exact lossless** for every image whose unique-color count fits the
index width; images above the 16-bit palette cap (65536 colors, e.g. photos) quantize
by format necessity, not by this work.

## Findings (2026-06-15 investigation)

Empirical root-cause work on real committed examples (`concept-map-uml-diagrams-overview`,
`5c-marketing-analysis`, `infographic-project-steps`, `class-diagram-example`,
`board-visual-tutorial`, `bpmn-2-example`). Measurement harness lived in Python decode +
numpy diff, but **the fix belongs in `dif-core`** (the check is part of the conversion,
not an external tool).

- **Light theme is lossless on diagrams.** Decoded light vs source: `bad_px = 0`,
  `maxdiff = 0` on every diagram tested. The only non-lossless files are 6 usc-sipi photos
  whose unique-color count (e.g. 230427) exceeds the 16-bit palette cap -> inherent
  quantization (`maxdiff <= 17`). "Light looks wrong" was the dark banding fooling the eye.

- **The per-color arithmetic transform is discontinuous at the chroma threshold.** Grays
  take `L' = 1 - L` (a flip: white->black). Faintly-tinted near-neutrals take
  `dark_lightness(L)` (a compress: white->gray, no flip). At the same lightness these
  disagree massively (`1-L = 0.05` vs `compress = 0.725`), and a 1-LSB chroma difference
  decides which. Demonstrated cliff:
  `(255,255,255) -> (0,0,0)` but `(254,255,255) -> (173,174,174)`;
  `(250,250,250) -> (0,0,0)` but `(252,250,252) -> (173,171,173)`.
  This single discontinuity produces: text-AA black halos (the `(3,3,2)` fringe of black
  text stays black while the `(0,0,0)` body flips to white), white-ish gradient
  punch-through (neutral highlights inside a tinted diamond flip to black), and gradient
  banding. The chroma threshold is a fixed global constant; the phenomenon has no fixed
  scale. **Retuning the constant cannot fix it; the branch itself is the defect.**

- **No global flip-vs-compress policy works.** "Foreground always compress" makes black
  text compress to near-black (invisible). "Background always flip" sends vivid colored
  fills to near-black (most diagrams have a transparent canvas, so the visible fills are
  the "background"). The correct dark color for a pixel is not a function of its color or
  its Bg/Fg role alone; it follows from **keeping the rendered structure intact** -- which
  is the job of the structural relaxation, not a hand-tuned transform.

- **The window-based AA detector was itself defective** (see the AA-detection phase below):
  a fixed `5x5` window plus a "two most-separated colors on-segment" test. It (a) skips
  endpoint colors, so a specular highlight chosen as a window endpoint is never filtered
  and falls through to the broken flip; and (b) never measures band width, so it cannot
  actually distinguish a 1px AA seam from a wide gradient. `WIN = 5` is the same class of
  fixed-scale defect as `chroma < 1e-3`.

- **The structural filters existed but were disconnected from AA detection.** Sobel lived
  in `aa_detect` only as a boolean gate; the Sobel+Laplacian combined map lived in
  `edgemap` only inside the optimizer loss. AA detection itself used the window heuristic,
  not the filters. The corrected design uses the filters to **find** the edge.

## Approach

### New / changed files

- `crates/dif-core/src/aa_detect.rs` -- OKLab channel build, Sobel/Laplacian edge energy,
  **filter-driven** AA detection (gradient-march to plateau/extremum endpoints + band-width
  thinness test), per-pixel AA endpoint/`t` data.
- `crates/dif-core/src/regions.rs` -- connected components on the index plane, component
  features, Foreground/Background labeling.
- `crates/dif-core/src/edgemap.rs` -- Laplacian energy, combined Sobel+Laplacian edge map,
  structural loss (`structural_loss`, `structural_loss_vs`).
- `crates/dif-core/src/optimize.rs` -- adjacent-pair collection, shared `gamut_clamp_oklab`,
  bounded relaxation of the dark palette against the structural loss.
- `crates/dif-core/src/derive.rs` -- region-parameterized derivation + AA blend + shared
  `gamut_clamp_oklab`.
- `crates/dif-core/src/lib.rs` -- composite-key split path, feature/capacity merge,
  optimizer wiring, reporting fields.
- `crates/dif-py/src/lib.rs` -- `add_dark_theme_regional`, `quantized`/`source_colors`.
- `py/dif_tools/convert.py` -- route `strategy="arithmetic"` through the regional path.

### Phase 0 -- Region-parameterized transform

`derive.rs`: `enum RegionClass { Background, Foreground }`; `arithmetic_rgb_region(.., class)`
(Background reproduces the historical math byte-for-byte; Foreground lifts derived dark
lightness for contrast, `FG_CONTRAST_K = 1.1`); `dark_color_for`;
`derive_dark_palette_regional`. Shared `gamut_clamp_oklab` factored out (byte-identical).

### Phase 1 -- Region labeling (`regions.rs`)

Union-find 4-connectivity components on the index plane (deterministic). Per-component
features: `area_fraction`, `thinness = area/perimeter`, OKLab `chroma`, OKLab `dL` contrast
vs the dominant differing neighbor. Scored vote (`>= 2` of 4) -> Foreground.

### Phase 2 -- Text mask (`lib.rs::grow_text_mask`) -- CURRENT

The live path does NOT use per-pixel AA detection. Per-pixel filtering could not reliably
catch text (it half-detected the fringe, left glyph extrema and missed fringe to the broken
per-color flip). Instead text is found by REGION:

- The region pass (`classify_regions`) finds glyph / thin-stroke **cores** (Foreground).
- The core is GROWN into its surrounding anti-aliasing shell, but only across **edge**
  pixels (Sobel energy above `TAU_EDGE`), never into flat fill (`grow_text_mask`, radius
  `TEXT_GROW = 2`). So the whole glyph plus its 1-2px AA shell is one text region while
  adjacent solid fills stay background.

`aa_detect.rs` (the filter-driven detector), `optimize.rs` (structural relaxation), and
`edgemap.rs` (structural loss) are RETAINED but **not on the live `build_regional` path** --
they are used only by the older `add_dark_theme_regional`. The optimizer was abandoned for
the live path because it pushed near-neutral text indices to gamut corners = false colors
(`255,0,255` magenta on text), which violates the "no false color" rule.

### Phase 3 -- Split by `(color, text?)` (`lib.rs::build_regional`)

Composite key `(color_id, class)` where class is `Foreground` (text) or `Background`. Split:
every `(color, class)` is a candidate index; the **light** RGBA is always the pixel's
original color (lossless light preserved). Merge: collapse candidates with near-identical
dark OKLab (`MERGE_EPS`), then capacity-merge to fit the index width (never grow width).

### Phase 4 -- Dark seed per class (`derive.rs`)

- **Background**: the per-color arithmetic transform (`dark_color_for`, Background) --
  vivid fills keep hue + compress, neutral page flips.
- **Text (Foreground + grown AA shell)**: `text_dark_for` = a CONTINUOUS lightness
  inversion `L' = 1 - L` keeping hue (OKLab `a,b`), with **no chroma branch**. Applied to
  the glyph core AND its fringe so the inverted glyph is a smooth shape (the chroma-branch
  discontinuity in the per-color transform is what split a glyph's fringe into
  flip-to-white and compress-to-dark pixels = the mottled, un-OCR-able text).

Result (measured): inversion fraction 0.98-1.00 and text/background contrast roughly
doubled vs legacy; `concept-map` went from negative contrast (unreadable) to clearly
readable. Light bit-exact, false-color count 0.

### Phase 5 -- (parked) structural relaxation

The structural optimizer (`edgemap.rs` + `optimize.rs`) is parked: it produced false colors
on text. If banding repair is ever wanted it must be **constrained to lightness only**
(lock `a,b`, or cap chroma at the seed) so it cannot create a hue. Not on the live path.

### Phase 6 -- Binding + Python wrapper

`Image::add_dark_theme_regional` runs the pipeline; no palette crosses FFI.
`convert.py` routes `strategy="arithmetic"` to the regional path (`regional=True` default),
keeps the legacy per-color path behind `regional=False` for A/B.

## Acceptance bar

**No regression vs legacy, on every metric, plus measurable improvement.** Objective,
non-eyeball checks:

- Light stays bit-exact on every diagram (`bad_px = 0`); photos quantize only by the cap.
- `structural_loss_vs(light, legacy, regional) <= structural_loss_vs(light, legacy, legacy)`
  on the test corpus (no new edges, no lost edges).
- Speckle metric: dark near-black opaque pixels whose light origin is a near-black AA fringe
  drops toward 0 (was 29244 on `concept-map`).
- Determinism: `to_difr` byte-equal across two builds.
- Visual confirmation on the named files (light vs legacy-dark vs regional-dark): smooth
  gradients, visible titles/separators, crisp text, no punch-through dots.

## Verification

- `just check && just clippy` after each Rust phase; `just test-encode` for units.
- `just py-test` after the binding phase.
- Full gate before claiming done: `just ci` (100% fn / >=80% line across `cov-all`) and
  `just py-ci` (per-file >=80% line) since the Python encode entry changed.
- `just regen-examples` (after `just py`) once the structural invariant + visuals pass,
  so committed `data/dif-examples/` match current code.

## Risks

- **Palette/index-width growth** -> split-then-merge with median-cut to capacity; keep
  determinism (packed-key tie-break).
- **Relaxation oscillation** -> Jacobi + geometric step decay + fixed iteration bound.
- **Optimizer cost on high-color diagrams** -> time bound, not a hard index cap; must stay
  fast + deterministic.
- **Coverage** -> new pub/loop branches (march endpoints, gradient reject, gamut clamp,
  relaxation lost/new/degenerate cases) need direct unit tests for 100% functions.

## Worklog

Folded in (formerly a separate `*.worklog.md`).

### 2026-06-15 -- setup
Branch `feat/region-aware-dark-derivation` off `main`. Plan staged under `docs/plan/`.

### 2026-06-15 -- Phase 0: region-parameterized transform
`derive.rs`: `RegionClass`, `FG_CONTRAST_K = 1.1`, `arithmetic_rgb_region` (Background
byte-identical), `dark_color_region`, `derive_dark_palette_regional`. `just test-encode`
30 passed.

### 2026-06-15 -- Phase 1: region labeling
`regions.rs`: `classify_regions` union-find components + scored Fg/Bg vote. Deterministic
neighbor tie-break. `just test-encode` 34 passed.

### 2026-06-15 -- Phase 2: AA detection (window-based -- later superseded)
First `aa_detect.rs`: directional walk to plateaus. Unstable endpoints on thin glyphs.

### 2026-06-15 -- Phase 3 + 4: split-then-merge + AA blend
`lib.rs` `add_dark_theme_regional` composite-key split; `derive.rs` `blend_oklab`,
`oklab_close`. `just test-encode` 46 passed.

### 2026-06-15 -- Phase 4.5: bounded-recursive edge-consistency
`derive.rs` `aa_blend_refined` (coarse-to-fine `t`, keep coarsest within `TAU_CONSISTENCY`).
`just test-encode` 47 passed.

### 2026-06-15 -- Phase 5/6 (first pass): binding + CI
`add_dark_theme_regional` binding, `convert.py` `regional=True`. `just ci` + `just py-ci`
green; 100% fn. Regenerated 103 examples (later found stale relative to subsequent fixes).

### 2026-06-15 -- Correction 1: window AA detector
Replaced the walk with a `5x5` most-separated-pair on-segment detector. Tests passed but
visual review still showed text speckle.

### 2026-06-15 -- Correction 2: legacy-init safety fallback
To stop regressions (erased `board` title at >4096 colors; text speckle), seeded every
composite key's dark to the legacy per-color value. Side effect: regional == legacy
byte-for-byte (no split seams, no AA blend) -> safe but a **no-op** (no improvement).

### 2026-06-15 -- Root-cause investigation (this session)
Established the findings above: light is lossless on diagrams; the per-color transform's
chroma-branch discontinuity is the single root cause of text halos, white-gradient
punch-through, and banding; no global flip-vs-compress policy works; the window AA detector
and the `WIN = 5` / `chroma < 1e-3` constants are fixed-scale defects. Decision: replace the
window detector with **filter-driven** AA detection (gradient-march to extrema + band-width
thinness test), undo the legacy-init no-op, and let the structural relaxation drive the dark
palette (it must run on high-color diagrams, not skip).

### 2026-06-16 -- Current design: preprocess-first + text-mask inversion
Replaced the two-step (`indexed_from_rgba8` + `add_dark_theme_regional`) with
`build_regional`: classify the RAW pixels first (region core + edge-constrained text-mask
growth), build the light index split by `(color, text?)`, then seed the dark theme --
Background via the per-color transform, Text via `text_dark_for` (continuous `L' = 1 - L`,
keep hue). AA snap and AA blend both abandoned (snap gave jagged text; blend + optimizer gave
false colors). The optimizer + filter-`detect_aa` are retained but off the live path. Python
`regional_from_rgba8` routes the regional default here. Measured: text inversion 0.98-1.00,
contrast roughly doubled, light bit-exact, false-color 0.

Accepted by the owner as "nearest to the ideal" -- the text color transform is the continuous
L-inversion (keep hue); this is the intended behavior, not a placeholder.

### Open defects (2026-06-16, owner-reported)
1. `5c-marketing-analysis` -- heavy banding (gradient fills posterize in dark).
2. `class-diagram-example` -- black holes in the enclosed counters of glyphs (the inside of
   `o`/`e`/`a`/`p` etc renders as a black hole instead of following the text).
3. `CSVimport-examples` -- a light-yellow fill becomes near-black in dark.

### Known follow-ups
- Code-review finding: `build_regional` inverts ALL Foreground, including thin colored
  lines / arrows (case 4), via the same `text_dark_for`. Decide whether lines should invert
  or compress.
- `aa_detect`/`optimize`/`edgemap` are now dead relative to the shipping path (only
  `add_dark_theme_regional` uses them). Decide: delete or keep.
- Coverage: 1 pre-existing uncovered function in `lib.rs` (line-only `just` recipes can't
  name it); new code is fully covered. Full `just ci` + `just py-ci` still to run.
