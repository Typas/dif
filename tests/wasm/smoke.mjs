// Wasm decoder smoke test under node. Loads the wasm-bindgen glue (its bare
// `wasi_snapshot_preview1` import is satisfied by the resolve hook in
// wasi-resolve.mjs), decodes web/flowchart.dif, and checks the decode + render.
// Exits non-zero on any mismatch so `just wasm-test` fails loudly.
import { readFile } from "node:fs/promises";
import init, { Image } from "../../web/pkg/dif_wasm.js";

const wasm = await readFile(new URL("../../web/pkg/dif_wasm_bg.wasm", import.meta.url));
// node's fetch can't load file:// URLs, so hand init the bytes directly.
await init({ module_or_path: wasm });

const dif = await readFile(new URL("../../web/flowchart.dif", import.meta.url));
const img = Image.fromBytes(new Uint8Array(dif));

const themes = img.themeNames().split("\n").filter(Boolean);
const rgba = img.render("light", 0);

function check(cond, msg) {
  if (!cond) {
    console.error(`wasm-test FAIL: ${msg}`);
    process.exit(1);
  }
}

check(img.width > 0, `width=${img.width}`);
check(img.height > 0, `height=${img.height}`);
check(themes.length > 0, "no theme names");
check(
  rgba.length === 4 * img.width * img.height,
  `render len ${rgba.length} != ${4 * img.width * img.height}`,
);

console.log(
  `wasm-test OK: ${img.width}×${img.height}, frames=${img.frameCount}, themes=[${themes.join(", ")}]`,
);
