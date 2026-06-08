// Shared decode + render + theme helpers for the DIF browser pages (the single
// demo in main.js and the examples gallery in gallery.js). The decoder is built
// to dist/pkg (`just wasm`); `just demo-build` rewrites this relative import to
// ./pkg when staging. The wasi import is satisfied by the import map in the HTML.
import init, { Image } from "../../dist/pkg/dif_wasm.js";

let ready = null;

// Initialise the wasm decoder once; repeated calls await the same promise.
export function initDecoder() {
  if (!ready) ready = init();
  return ready;
}

// Parse a CSS color like "rgb(18, 18, 18)" / "rgba(...)" into [r,g,b]. The host
// background tie-breaks between equally-capable themes (v3 pick_theme).
export function parseRgb(css, fallback) {
  const m = /rgba?\(([^)]+)\)/.exec(css || "");
  if (!m) return fallback;
  const p = m[1].split(",").map((s) => parseInt(s.trim(), 10));
  return p.length >= 3 && p.every((n) => Number.isFinite(n)) ? [p[0], p[1], p[2]] : fallback;
}

// Page background/foreground per appearance. Driving the whole page from the
// effective mode gives a true light/dark page -- the CSS @media rule alone only
// follows the OS, so an explicit override would re-theme the canvas but not the page.
export const PAGE_THEME = { light: ["#ffffff", "#111111"], dark: ["#1e1e1e", "#eeeeee"] };

export function applyPageTheme(mode) {
  const [bg, fg] = PAGE_THEME[mode] || PAGE_THEME.light;
  document.body.style.background = bg;
  document.body.style.color = fg;
}

// Mount a canvas viewer. `show(bytes)` decodes a .dif and paints it; `setMode`
// re-themes the current image. Handles multi-frame animation (per-frame us
// delays, replay_count; 0 = infinite). Call applyPageTheme(mode) before setMode
// so the host-background tie-break reads the updated page background.
export function mountViewer(canvas) {
  const ctx = canvas.getContext("2d");
  let img = null;
  let mode = "light";
  let timer = null;

  function hostBase() {
    const fallback = mode === "dark" ? [0, 0, 0] : [255, 255, 255];
    return parseRgb(getComputedStyle(document.body).backgroundColor, fallback);
  }

  function paint(base, frame) {
    const [r, g, b] = base;
    const rgba = img.render(mode, r, g, b, frame);
    const data = new ImageData(new Uint8ClampedArray(rgba), img.width, img.height);
    ctx.putImageData(data, 0, 0);
  }

  function draw() {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    if (!img) return;
    const base = hostBase();
    const n = img.frameCount;
    if (n <= 1) {
      paint(base, 0);
      return;
    }
    let loopsLeft = img.replayCount; // 0 => infinite
    let frame = 0;
    const step = () => {
      paint(base, frame);
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

  return {
    show(bytes) {
      img = Image.fromBytes(bytes);
      canvas.width = img.width;
      canvas.height = img.height;
      draw();
      return img;
    },
    setMode(next) {
      mode = next;
      draw();
    },
  };
}
