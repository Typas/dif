// Single-image demo: decode flowchart.dif and recolor it to match the OS theme,
// with an override button. Decode/render/theme logic lives in viewer.js, shared
// with the examples gallery (gallery.js).
import { initDecoder, mountViewer, applyPageTheme } from "./viewer.js";

const DIF_URL = "./flowchart.dif";

async function main() {
  await initDecoder();

  // Cache-bust: the dev server sends no Cache-Control, so a bare URL gets
  // served from the browser cache even across hard reloads.
  const resp = await fetch(`${DIF_URL}?t=${Date.now()}`);
  if (!resp.ok) throw new Error(`failed to fetch ${DIF_URL}: ${resp.status}`);
  const bytes = new Uint8Array(await resp.arrayBuffer());

  const viewer = mountViewer(document.getElementById("view"));
  const modeLabel = document.getElementById("mode");
  const info = document.getElementById("info");

  const media = window.matchMedia("(prefers-color-scheme: dark)");
  let override = null; // null = follow OS; otherwise "light" | "dark"

  function currentMode() {
    if (override) return override;
    return media.matches ? "dark" : "light";
  }

  function refresh() {
    const mode = currentMode();
    applyPageTheme(mode);
    viewer.setMode(mode);
    modeLabel.textContent = mode + (override ? " (override)" : "");
  }

  refresh(); // set the mode first (no image yet, so this is a no-op paint)
  const img = viewer.show(bytes); // decode + paint at the current mode
  info.textContent = `${img.width}*${img.height}, themes: ${img.themesDescription().split("\n").join(", ")}`;

  media.addEventListener("change", refresh);
  document.getElementById("toggle").addEventListener("click", () => {
    override = currentMode() === "dark" ? "light" : "dark";
    refresh();
  });
}

main().catch((err) => {
  document.body.insertAdjacentHTML("beforeend", `<pre style="color:#c00">${err}</pre>`);
  console.error(err);
});
