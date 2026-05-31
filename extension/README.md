# DIF Viewer

A theme-aware custom editor for **`.dif` / `.difr`** (Diagram Image Format) files.
Opening one renders it in a webview via the `dif-wasm` decoder and re-renders to
match the editor's active color theme (light / dark / high-contrast).

## Build & install

From the repository root:

```sh
just ext-build              # stage the wasm decoder + compile the TypeScript
just ext-package            # build dif-viewer.vsix into the repo root
just ext-install            # package + install via `code` (default)
just ext-install codium     # ...or another editor binary: codium, cursor, ...
```

`ext-install` runs `<variant> --install-extension dif-viewer.vsix`. To install
through the GUI instead — works in any VS Code-family editor (VS Code, VSCodium,
Cursor, …):

> **Extensions** view ▸ **⋯** menu ▸ **Install from VSIX…** ▸ pick `dif-viewer.vsix`
