# Multi-Frame Implementation

Date: 2026-06-02

## Status

Implemented (core + in-frame split). Inter-frame parallel encode, parallel decode, and the
zstd/brotli in-frame split (`k`-rule, jobSize `J`) are wired in `crates/dif-core/src/codec.rs`
(`compress_frames`, `parallel_map`, `from_dif_workers`) and exposed via the `--threads` CLI
flag. Deferred: tail-wave backfill (look-ahead indexing, `codec_frame` level bump) and the
ratio/speed mode flags. Note: the per-family table below understated brotli --- `brotli-mt`'s
`compress_multi` is a single-blob native-MT path, so brotli (`level >= 10`) is also
split-eligible (at a higher ratio cost than zstd, so biased conservative), not `k = 1`.

Grounded in `docs/spec/dif-spec.typ` (v3 container, two-stage body, three codec slots,
`frame_alignment`-strided frames).

## Current baseline (anti-pattern - the thing to beat)

Today the encoder flushes the whole thread budget straight into the codec
(`nbWorkers = T`, currently 12) for whatever it is compressing, with no frame- or
size-awareness. zstd then splits the stream by its own internal jobSize (a small ~1 MB
floor, **not** the window-derived size). Consequences, confirmed on the single-frame 16b
corpus:

- Every multi-MB frame gets chopped into many jobs whether or not it helps - uncontrolled
  over-split. Measured intra-frame ratio cost up to ~+2% (zst3, redundant images such as
  `moon-habitat`), shrinking at higher levels (the optimal parser re-finds severed
  matches). brotli over-splits even sub-KB inputs (framing overhead -> +167% on
  `gray21.512.tiff`).
- Inter-frame parallelism is **not used at all**; with one frame, all 12 workers are forced
  intra-frame - the worst case for the cost above.

This baseline is a bug, not a reference. The design below is judged on its own merits; the
benchmark only supplies the *cost curve* (redundancy-driven, <=2%, self-heals with level)
that sets how hard the cap should bite. Do not calibrate k or jobSize to reproduce current
behavior.

## Problem

A multi-frame `.dif` has `frame_count` frames, all `width x height`, sharing one global
palette section. The body is two-stage (spec "Body layout"):

- `codec_palette` compresses the single concatenated palette section once.
- `codec_frame` compresses each frame's index plane **independently**.
- the outer `codec` wraps the whole intermediate body.

Each frame's index plane is `S = width * height * i` bytes, `i = 1` (8-bit index) or `2`
(16-bit). The palette holds the RGBA colors, so a frame stream is just indices - already
the smallest representation; channel count and color depth live in the palette, not the
frame.

We have a fixed thread budget `T` and two ways to spend it on the frame sections:

- **Inter-frame parallelism** - compress different frames on different threads. Frames are
  independent `codec_frame` streams (spec: "Each frame is compressed independently"), so
  this is *free*: no ratio loss, no decode penalty.
- **In-frame parallelism** - split one frame's index plane into multiple codec jobs. This
  *costs* ratio (lost cross-job matches) **and** decode speed (each job is an independent
  sub-stream to re-init). Severity scales with the codec family's window vs the job size.

When `frame_count >= T`, inter-frame alone fills the pool with zero cost, so in-frame
splitting is pure waste. We want in-frame used only when forced by too few frames, with any
loss confined to a small tail.

## Goal

- Saturate `T` with the free inter-frame path whenever `frame_count >= T`.
- Use in-frame splitting only as a forced fallback; bound its cost to one tail wave.
- Keep the outer codec from serializing what the frame layout makes parallelizable.
- Deterministic per thread count: same input + same `T` reproduces.

## Format hooks we rely on

From the spec:

- **Independent frame streams.** `frames[j]` = `size:u64, delay:u32, compressed_content,
  padding` - each `compressed_content` is a standalone `codec_frame` stream. Compress and
  decompress per frame in parallel.
- **Uniform stride.** Frame `j` starts at `first_frame_offset + j * frame_alignment`
  (multiple of 16), measured **into the intermediate body** (spec: offset is "into the
  outer-decompressed intermediate body"). O(1) seek to any frame within the intermediate
  body => parallel decode needs no index scan. Cost: `frame_alignment` must cover the
  largest compressed frame, so smaller frames pad - the space price of seekability.
- **Outer codec is a wrapper, not a parallelism gate.** The stride offsets always address
  the intermediate body, so per-frame parallel (de)compression operates on the intermediate
  body whatever the outer codec is. What the outer codec changes is *how you get* the
  intermediate body:
  - outer = `Store`: `compressed_body` **is** the intermediate body, so file offsets equal
    intermediate offsets - seek each frame directly in the file, decode it without
    materializing the rest (low memory, no serial pass).
  - outer != `Store`: one serial inflate of the whole `compressed_body` reconstructs the
    intermediate body in memory first; frames are then still seekable and parallel within
    it, but you pay a serial whole-body pass and hold the full intermediate body resident.

## Pipeline (encode)

```
1. quantize       raw pixels -> global palette (+ index width)   [encoder detail, barrier]
2. index          each frame's pixels -> index plane (W*H*i)     [per-frame parallel, free]
3a. palette sect  compress palette section with codec_palette    [one task]
3b. frame sects   compress each index plane with codec_frame     [inter-frame parallel]
4. outer          wrap intermediate body with codec              [Store => no-op + parallel]
```

Palette build (step 1) is a barrier (needs colors from all frames; sampling/quantization is
encoder-side, not in the format). After it, steps 2-3b are per-frame independent. Pipeline
per frame: as a frame is indexed, queue it for `codec_frame` compression.

Recommend **outer `codec` = Store** for multi-frame: the section codecs already compress, so
the intermediate body equals the file - frames are seekable directly in the file and decode
one at a time (low memory, no serial outer pass). A non-Store outer buys little
(recompressing already-compressed sections) while forcing a serial whole-body inflate and a
full resident intermediate body on decode. Frame parallelism itself holds either way.

## Thread allocation (the k rule)

All frames identical size => no skew => static partition is optimal; no work-stealing.

Let `S = width * height * i` (frame index-plane bytes). `J` = the **controlled** per-job
size the scheduler sets and passes to the codec - a design knob, **not** the codec's
internal default. (The window-derived estimate used earlier was wrong: zstd's real internal
jobSize floor is ~1 MB, so left to itself the codec over-splits any multi-MB frame. We set
`J` explicitly instead of predicting it.) Split factor per frame:

```
k = 1                              if frame_count >= T
k = min(ceil(T/frame_count),      if frame_count < T
        floor(S/J))
```

Each frame splits into `k` equal jobs => `frame_count * k` equal jobs => perfect balance.

- **frame_count >= T -> k = 1.** Pure inter-frame. Zero ratio loss, fastest decode. Target.
- **frame_count < T -> k > 1.** Too few frames to fill `T`; split to use the rest. Cost
  scales with `k`, gated by `floor(S/J)` so we never make more jobs than the stream fills.

`J` is also the **cap that stops over-split**: by choosing `J` large (few, big jobs) we
keep cross-job matches and bound the ratio tax; choosing it small trades ratio for more
parallelism. This is the lever the current baseline lacks - it lets the codec pick a tiny
`J` and over-chop. Set `J` from the measured cost curve: large enough that the per-frame
ratio loss stays within budget (the corpus showed <=2% even at the small default, shrinking
with level), small enough to fill idle threads when `frame_count < T`.

Note the split *count* `floor(S/J)` and the ratio *cost* are different axes: a frame can
split (k>1) yet lose almost nothing if its content has few cross-job matches
(`story-map` split but moved -0.07%), while a redundant frame of similar size pays the full
tax (`moon-habitat` +2%). Size gates whether we split; redundancy sets what it costs.

## Per-family in-frame feasibility

The format stores **one** `compressed_content` blob per frame, so an in-frame split must
still yield a single decodable blob. That constrains which families can split at all:

- **Native-MT, single-blob** (`zstd`): codec-internal workers produce one valid stream with
  internal job boundaries -> `k > 1` fits the format directly.
- **No native MT** (`brotli`, `zxc`, `lz4`, `lzav`, DEFLATE): a manual split would need
  concatenated sub-streams with private framing, which the single-blob frame record does not
  carry. So these stay **`k = 1`** (inter-frame only). Not a loss: their windows are small
  (DEFLATE 32 KB, lz4 64 KB, zxc 64 KB, lzav small) so in-frame split would barely help
  ratio anyway - and brotli's 16 MB window is exactly the family that suffers most from
  splitting, so forcing it to `k = 1` is the right call.

Net: in-frame split is a zstd-only fallback, used only when `frame_count < T` and
`S >= 2J`. Everything else relies on inter-frame parallelism, which the format already
gives for free.

## Tail-wave amortization

Process frames in waves of `T`:

- Full waves (`T` frames) -> `k = 1`, no loss.
- Only the remainder wave (`r = frame_count mod T`) is under-filled -> `k = ceil(T/r)`
  there (zstd frames only). Loss confined to one partial wave; with `frame_count >> T`,
  negligible.

### Spare-thread priority (under-filled tail)

```
while threads idle in tail wave:
  1. compress any frame still queued        (k=1)             - free
  2. index a look-ahead frame                                 - free, feeds step 1
  3. raise codec_frame level on an in-flight frame            - spends cycles on ratio
  4. in-frame split (k>1, zstd frames only)                   - lossy fallback
```

The palette is built once (shared, global), so palette quantization is **not** available as
tail backfill. Useful backfills are look-ahead indexing (2) and a `codec_frame` level bump
(3) - both strictly better than splitting.

## Decode parallelism

Parallel frame decode runs on the **intermediate body**; the outer codec only decides how
that body is obtained (see "Format hooks").

1. Reconstruct the intermediate body:
   - outer = `Store`: no work - `compressed_body` already is it; seek frames directly in the
     file, decode each without materializing the rest (low memory).
   - outer != `Store`: one serial inflate of the whole `compressed_body` -> full
     intermediate body resident in memory.
2. Decode the palette section once (`codec_palette`), shared by all frames.
3. Decode frames in parallel: frame `j`'s `compressed_content` at `first_frame_offset +
   j * frame_alignment` within the intermediate body, each an independent `codec_frame`
   stream. No index scan, no inter-frame dependency.

So parallelism holds for any outer codec; outer = `Store` is preferred because it skips the
serial step-1 inflate and keeps memory to one frame at a time. `k > 1` (split) frames cost
extra decode (per-sub-stream re-init); only zstd frames ever split, and only in the tail, so
the bulk is single-stream-per-frame and fully parallel. Ratio-optimal and decode-optimal
policies agree: bias hard to inter-frame.

## Knobs

- outer `codec`: Store for multi-frame parallel (recommended) vs a real codec (serial,
  marginal extra ratio).
- `codec_frame` jobSize `J`: sets the in-frame split threshold (zstd). Smaller = more
  splittable, worse ratio.
- mode:
  - **ratio/decode mode** - cap `k = 1` always; let tail threads idle. Often better when
    `frame_count` is near `T`, since in-frame split hurts both ratio and decode.
  - **speed mode** - split freely (`k` per formula, zstd frames).
- index width (`i`): from `index_count` per spec; fixes `S = W*H*i`.

## Edge cases

- `frame_count = 1` -> single frame; `k = min(T, floor(S/J))`, zstd only, else serial.
- Non-zstd `codec_frame` + `frame_count < T` -> threads idle (no legal in-frame split);
  spend them on look-ahead indexing or a level bump, not splitting.
- `frame_alignment` sizing: must cover the largest compressed frame; large variance =
  padding waste. Choose per-file as `align16(max_j compressed_size_j)`.
- Memory: two-pass (subsample for palette, then index+compress) bounds resident frames;
  one-pass only if all frames fit.
- Look-ahead buffer for tail backfill must be bounded.

## Open questions

- Palette sampling rate in the quantize barrier: fixed fraction vs adaptive to color
  variance?
- Hard barrier vs overlap quantize with early-frame indexing (refine palette as frames
  arrive). Current plan: hard barrier for determinism.
- Does any future `codec_frame` gain a single-blob native-MT path (like zstd) and so become
  eligible for `k > 1`? Re-check this table when codecs are added.

## Worklog

### 2026-06-04 --- core + in-frame split landed

Shipped the inter-frame scheduler, parallel decode, and the in-frame split fallback. All
`just build`/`build-std`/`build-native`, `just test` (23), `just py-test` (64), `just clippy`,
and `just py-lint` pass. Coverage: Rust 93.5% line (`just cov`), Python 85% (`just py-cov`).

**Built**

- `crates/dif-core/src/codec.rs`:
  - `parallel_map(n, threads, f)` --- `std`-gated bounded pool (`std::thread::scope` + an
    `AtomicUsize` work-queue), serial fallback under no_std or `threads < 2`. Results indexed
    by item, so output is byte-identical to serial regardless of thread count.
  - `compress_frames` --- replaces the serial frame loop. `frame_count >= T` -> pure inter-frame
    (`k = 1`, each codec serial via `workers = 0`); `frame_count < T` -> split each frame
    `k = min(ceil(T/n), floor(S/J))`, one thread per frame.
  - `compress_frame` --- split dispatch: zstd (`NbWorkers = k` + new `CParameter::JobSize(J)`),
    brotli (`workers = k`, level >= 10 -> `compress_multi`), all others forced `k = 1`.
  - `zstd_compress` gained a `job_size: Option<u32>` arg; decode loop now runs through
    `parallel_map`; new public `from_dif_workers(bytes, workers)` (`from_dif` stays serial).
- Bindings/CLI: `--threads` flag (default **1**) -> `to_dif(workers)`; `from_dif(data,
  workers=1)`; stub + `convert.py`/`__main__.py` plumbing.
- `justfile`: added `cov-missing` (line-level gap report).

**Deviations from the design above**

- **Brotli is split-eligible too**, not zstd-only. `brotli-mt`'s `compress_multi` emits a
  single decodable blob (parallel meta-blocks), so brotli at `level >= 10` takes `k > 1` (at a
  higher ratio cost than zstd --- 16 MB window --- so biased conservative). The "Per-family
  in-frame feasibility" section above understates this; treat that table as superseded.
- **Threading is a bounded `std::thread::scope` pool, not literal `T`-frame waves.** The
  `k`-rule is applied globally (all frames equal size), which collapses to the same allocation
  the wave framing describes; the wave model was the memory-bounding lens, deferred with the
  rest of tail handling.
- **`J` is a `const DEFAULT_FRAME_JOB_SIZE = 4 MiB`, not yet a user knob.** Promote to a
  parameter when calibrating against the 16b corpus.
- **Outer codec default unchanged** (kept `zstd-3`); Store is only *recommended* for
  multi-frame in the `--codec` help, not auto-selected.

**Deferred** (not in this pass): tail-wave backfill (look-ahead indexing, `codec_frame` level
bump), ratio/speed mode flags, exposing `J` as a knob, and calibrating `J` against the corpus.

**Tests added** (`codec.rs` + `py/tests/test_codec.py`): serial<=>parallel encode/decode byte
parity; zstd and brotli in-frame-split blob roundtrips; no-split-codec roundtrip under thread
pressure; corrupt-frame-record rejection on both serial and parallel decode (error
propagation through the pool); Python multi-frame `--threads` parity.
