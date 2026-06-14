// Decode-speed benchmark for the wasm decoder, under node. Mirrors smoke.mjs's
// module load (the bare `wasi_snapshot_preview1` import is satisfied by the
// resolve hook in wasi-resolve.mjs), then time-budgets two phases per input:
//   parse  --- Image.fromBytes(bytes)         (container parse + decompress)
//   render --- img.render(mode, r, g, b, 0)   (theme pick -> packed RGBA8 + copy-out)
//
// Each parsed Image is wasm-bindgen-owned, so the parse loop frees every handle
// (otherwise GC pressure skews the timing); the render loop reuses one Image.
// Usage: node --import .../wasi-resolve.mjs bench.mjs [mode] [file.dif ...]
//   mode defaults to "light"; files default to web/demo/flowchart.dif.
import { readFile } from "node:fs/promises";
import init, { Image } from "../../dist/pkg/dif_wasm.js";

const wasm = await readFile(new URL("../../dist/pkg/dif_wasm_bg.wasm", import.meta.url));
await init({ module_or_path: wasm }); // node's fetch can't load file://, pass bytes.

const args = process.argv.slice(2);
const mode = args[0] && !args[0].endsWith(".dif") && !args[0].endsWith(".difr") ? args.shift() : "light";
const files = args.length ? args : [new URL("../demo/flowchart.dif", import.meta.url).pathname];

const WARMUP_MS = 100; // discard: wasm compile + JIT settle.
const BUDGET_MS = 400; // measured window per phase.

// Run `fn` for ~budgetMs after a warmup, return mean ms/op over the timed window.
function timed(fn, budgetMs) {
  let t = performance.now();
  while (performance.now() - t < WARMUP_MS) fn();
  let iters = 0;
  t = performance.now();
  let elapsed;
  do {
    fn();
    iters++;
    elapsed = performance.now() - t;
  } while (elapsed < budgetMs);
  return elapsed / iters;
}

const pad = (s, n) => String(s).padEnd(n);
const padL = (s, n) => String(s).padStart(n);
console.log(`wasm decode bench (mode=${mode}, warmup=${WARMUP_MS}ms, window=${BUDGET_MS}ms)\n`);
console.log(`${pad("file", 38)} ${padL("WxH", 11)} ${padL("KiB", 8)} ${padL("parse ms", 10)} ${padL("render ms", 10)} ${padL("Mpx/s", 8)}`);

for (const file of files) {
  const bytes = new Uint8Array(await readFile(file));

  const parseMs = timed(() => Image.fromBytes(bytes).free(), BUDGET_MS);

  const img = Image.fromBytes(bytes);
  const { width: w, height: h } = img;
  const renderMs = timed(() => img.render(mode, 255, 255, 255, 0), BUDGET_MS);
  img.free();

  const name = file.replace(/^.*\//, "");
  const mpxPerS = w * h / (renderMs / 1000) / 1e6;
  console.log(
    `${pad(name, 38)} ${padL(`${w}x${h}`, 11)} ${padL((bytes.length / 1024).toFixed(1), 8)} ` +
      `${padL(parseMs.toFixed(4), 10)} ${padL(renderMs.toFixed(4), 10)} ${padL(mpxPerS.toFixed(1), 8)}`,
  );
}
