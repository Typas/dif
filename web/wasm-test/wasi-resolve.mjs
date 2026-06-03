// ESM resolve-hook preload: map the wasm-bindgen glue's bare
// `wasi_snapshot_preview1` import to web/wasi_shim.js. The browser/extension
// does this with an import map; node has none, so we register a loader hook.
// Preload with: node --import tests/wasm/wasi-resolve.mjs ...
import { register } from "node:module";

register("./wasi-resolve-hook.mjs", import.meta.url);
