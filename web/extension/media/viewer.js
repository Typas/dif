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

// Parse "#rgb", "#rrggbb", or "rgb(r,g,b)" into [r,g,b], else `fallback`.
function parseColor(css, fallback) {
  const s = (css || "").trim();
  let m = /^#([0-9a-f]{3})$/i.exec(s);
  if (m) return [...m[1]].map((c) => parseInt(c + c, 16));
  m = /^#([0-9a-f]{6})$/i.exec(s);
  if (m) return [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16));
  m = /rgba?\(([^)]+)\)/.exec(s);
  if (m) {
    const p = m[1].split(",").map((x) => parseInt(x.trim(), 10));
    if (p.length >= 3 && p.every(Number.isFinite)) return [p[0], p[1], p[2]];
  }
  return fallback;
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
  // kinds map to the high-contrast capability; a file with no capable theme falls
  // back to theme 0 (per spec).
  function detectKind() {
    const c = document.body.classList;
    if (c.contains("vscode-high-contrast")) return "high-contrast";
    if (c.contains("vscode-dark")) return "dark";
    return "light";
  }

  // The editor background tie-breaks between equally-capable themes (v3). VS Code
  // exposes it as the --vscode-editor-background CSS var on <body>.
  function hostBase(kind) {
    const fallback = kind === "light" ? [255, 255, 255] : [0, 0, 0];
    const css = getComputedStyle(document.body).getPropertyValue(
      "--vscode-editor-background",
    );
    return parseColor(css, fallback);
  }

  let kind = detectKind();
  function draw() {
    const [r, g, b] = hostBase(kind);
    const rgba = img.render(kind, r, g, b, 0);
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
