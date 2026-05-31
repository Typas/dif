// Demo: decode a .dif in the browser and recolor it to match the OS theme.
import init, { Image } from "./pkg/dif_wasm.js";

const DIF_URL = "./flowchart.dif";

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
  info.textContent = `${img.width}×${img.height}, themes: ${img.themeNames().split("\n").join(", ")}`;

  const media = window.matchMedia("(prefers-color-scheme: dark)");
  let override = null; // null = follow OS; otherwise "light" | "dark"

  function currentMode() {
    if (override) return override;
    return media.matches ? "dark" : "light";
  }

  function draw() {
    const mode = currentMode();
    const rgba = img.render(mode, 0);
    const data = new ImageData(new Uint8ClampedArray(rgba), img.width, img.height);
    ctx.putImageData(data, 0, 0);
    modeLabel.textContent = mode + (override ? " (override)" : "");
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
