// The resolve hook (runs on a separate loader thread). Anything imported as
// `wasi_snapshot_preview1` resolves to the repo's web/wasi_shim.js.
const SHIM = new URL("../../web/wasi_shim.js", import.meta.url).href;

export async function resolve(specifier, context, nextResolve) {
  if (specifier === "wasi_snapshot_preview1") {
    return { url: SHIM, shortCircuit: true };
  }
  return nextResolve(specifier, context);
}
