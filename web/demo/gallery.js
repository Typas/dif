// Examples gallery: a sidebar lists every committed .dif (grouped by category
// from examples/index.json); clicking one decodes and shows it, one at a time.
// A fixed top-right Auto|Light|Dark switch re-themes the shown image and the
// page, persisted in localStorage. Decode/render/theme logic is shared (viewer.js).
import { initDecoder, mountViewer, applyPageTheme } from "./viewer.js";

const CATEGORY_LABELS = { drawio: "Diagrams", "usc-sipi-misc": "Photos" };
const THEME_KEY = "dif-theme";

function showError(err) {
  document.body.insertAdjacentHTML("beforeend", `<pre style="color:#c00">${err}</pre>`);
  console.error(err);
}

async function main() {
  await initDecoder();

  const resp = await fetch("./examples/index.json");
  if (!resp.ok) throw new Error(`failed to fetch examples/index.json: ${resp.status}`);
  const manifest = await resp.json();

  const viewer = mountViewer(document.getElementById("view"));
  const titleEl = document.getElementById("title");
  const infoEl = document.getElementById("info");
  const sidebar = document.getElementById("sidebar");
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  const themeButtons = [...document.querySelectorAll("#theme-switch button")];

  let pref = localStorage.getItem(THEME_KEY) || "auto"; // auto | light | dark

  function currentMode() {
    if (pref === "light" || pref === "dark") return pref;
    return media.matches ? "dark" : "light";
  }

  function applyTheme() {
    const mode = currentMode();
    applyPageTheme(mode);
    viewer.setMode(mode);
    for (const b of themeButtons) b.classList.toggle("active", b.dataset.mode === pref);
  }

  for (const b of themeButtons) {
    b.addEventListener("click", () => {
      pref = b.dataset.mode;
      localStorage.setItem(THEME_KEY, pref);
      applyTheme();
    });
  }
  media.addEventListener("change", () => {
    if (pref === "auto") applyTheme();
  });

  // Selection: fetch the chosen .dif, decode, and show it (replacing the prior).
  let selectedBtn = null;
  async function select(category, name, btn) {
    const r = await fetch(`./examples/${category}/${name}`);
    if (!r.ok) throw new Error(`failed to fetch ${name}: ${r.status}`);
    const img = viewer.show(new Uint8Array(await r.arrayBuffer()));
    titleEl.textContent = name.replace(/\.dif$/, "");
    infoEl.textContent = `${img.width}*${img.height}, themes: ${img.themesDescription().split("\n").join(", ")}`;
    if (selectedBtn) selectedBtn.classList.remove("active");
    selectedBtn = btn;
    btn.classList.add("active");
  }

  let first = null;
  for (const [category, files] of Object.entries(manifest)) {
    const h = document.createElement("h2");
    h.textContent = CATEGORY_LABELS[category] || category;
    sidebar.appendChild(h);
    for (const name of files) {
      const btn = document.createElement("button");
      btn.textContent = name.replace(/\.dif$/, "");
      btn.addEventListener("click", () => select(category, name, btn).catch(showError));
      sidebar.appendChild(btn);
      if (!first) first = { category, name, btn };
    }
  }

  applyTheme();
  if (first) await select(first.category, first.name, first.btn);
}

main().catch(showError);
