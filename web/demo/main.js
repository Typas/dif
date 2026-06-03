// Demo: decode a .dif in the browser and recolor it to match the OS theme.
// The decoder is built to dist/pkg (`just wasm`); serve the repo root so this
// relative path resolves. The wasi import is satisfied by the import map in
// index.html (resolved relative to the document, so wasi_shim.js stays here).
import init, { Image } from "../../dist/pkg/dif_wasm.js";

const DIF_URL = "./flowchart.dif";

// Parse a CSS color like "rgb(18, 18, 18)" / "rgba(...)" into [r,g,b]. The host
// background tie-breaks between equally-capable themes (v3 pick_theme).
function parseRgb(css, fallback) {
  const m = /rgba?\(([^)]+)\)/.exec(css || "");
  if (!m) return fallback;
  const p = m[1].split(",").map((s) => parseInt(s.trim(), 10));
  return p.length >= 3 && p.every((n) => Number.isFinite(n)) ? [p[0], p[1], p[2]] : fallback;
}

async function main() {
  await init();

  // Cache-bust: the dev server sends no Cache-Control, so a bare URL gets
  // served from the browser cache even across hard reloads.
  const resp = await fetch(`${DIF_URL}?t=${Date.now()}`);
  if (!resp.ok) throw new Error(`failed to fetch ${DIF_URL}: ${resp.status}`);
  const bytes = new Uint8Array(await resp.arrayBuffer());
  const img = Image.fromBytes(bytes);

  const canvas = document.getElementById("view");
  canvas.width = img.width;
  canvas.height = img.height;
  const ctx = canvas.getContext("2d");

  const modeLabel = document.getElementById("mode");
  const info = document.getElementById("info");
  info.textContent = `${img.width}×${img.height}, themes: ${img.themesDescription().split("\n").join(", ")}`;

  const media = window.matchMedia("(prefers-color-scheme: dark)");
  let override = null; // null = follow OS; otherwise "light" | "dark"

  function currentMode() {
    if (override) return override;
    return media.matches ? "dark" : "light";
  }

  // The host background color: the page's computed background, falling back to a
  // sensible default per appearance.
  function hostBase(mode) {
    const fallback = mode === "dark" ? [0, 0, 0] : [255, 255, 255];
    return parseRgb(getComputedStyle(document.body).backgroundColor, fallback);
  }

  // Animation: cycle frames honoring per-frame µs delays and replay_count
  // (0 = infinite). A single frame (or all-zero delays) just paints once.
  let timer = null;
  let loopsLeft = img.replayCount; // 0 => infinite

  function paint(mode, base, frame) {
    const [r, g, b] = base;
    const rgba = img.render(mode, r, g, b, frame);
    const data = new ImageData(new Uint8ClampedArray(rgba), img.width, img.height);
    ctx.putImageData(data, 0, 0);
    modeLabel.textContent = mode + (override ? " (override)" : "");
  }

  function draw() {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    const mode = currentMode();
    const base = hostBase(mode);
    const n = img.frameCount;
    if (n <= 1) {
      paint(mode, base, 0);
      return;
    }
    loopsLeft = img.replayCount;
    let frame = 0;
    const step = () => {
      paint(mode, base, frame);
      const delayMs = Math.max(img.frameDelay(frame) / 1000, 16);
      frame += 1;
      if (frame >= n) {
        frame = 0;
        if (img.replayCount !== 0 && --loopsLeft <= 0) return; // finished
      }
      timer = setTimeout(step, delayMs);
    };
    step();
  }

  media.addEventListener("change", draw);
  document.getElementById("toggle").addEventListener("click", () => {
    override = currentMode() === "dark" ? "light" : "dark";
    draw();
  });

  draw();
}

main().catch((err) => {
  document.body.insertAdjacentHTML("beforeend", `<pre style="color:#c00">${err}</pre>`);
  console.error(err);
});
