// Webview side: boot dif-wasm, decode the embedded .dif, and re-render on theme.
const cfg = window.__DIF;
const vscode = acquireVsCodeApi();
const errEl = document.getElementById("err");

function b64ToBytes(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

async function run() {
  const mod = await import(cfg.pkg);
  await mod.default(cfg.wasm);
  const img = mod.Image.fromBytes(b64ToBytes(cfg.b64));

  const canvas = document.getElementById("view");
  canvas.width = img.width;
  canvas.height = img.height;
  const ctx = canvas.getContext("2d");

  // VS Code tags <body> with the active theme class before any script runs, so
  // read it synchronously and render the right DIF theme on the first frame
  // instead of drawing "light" then swapping when the extension replies. Both HC
  // kinds carry vscode-high-contrast -> tag 2; a file lacking the requested tag
  // falls back to theme 0 (per spec).
  function detectKind() {
    const c = document.body.classList;
    if (c.contains("vscode-high-contrast")) return "high-contrast";
    if (c.contains("vscode-dark")) return "dark";
    return "light";
  }

  let kind = detectKind();
  function draw() {
    const rgba = img.render(kind, 0);
    const data = new ImageData(new Uint8ClampedArray(rgba), img.width, img.height);
    ctx.putImageData(data, 0, 0);
  }

  window.addEventListener("message", (e) => {
    const msg = e.data;
    if (msg && msg.type === "theme" && msg.kind !== kind) {
      kind = msg.kind;
      draw();
    }
  });

  draw();
  vscode.postMessage({ type: "ready" }); // ask the extension for the current theme
}

run().catch((err) => {
  errEl.textContent = String(err && err.stack ? err.stack : err);
});
