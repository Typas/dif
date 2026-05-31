# DIF Viewer

A theme-aware custom editor for **`.dif` / `.difr`** (Diagram Image Format) files.
Opening one renders it in a webview via the `dif-wasm` decoder and re-renders to
match the editor's active color theme (light / dark / high-contrast).

## Build & install

From the repository root:

```sh
just ext-build      # stage the wasm decoder + compile the TypeScript
just ext-package    # produce a .vsix
```

Install the resulting `extension/*.vsix` through the editor GUI — works in any
VS Code-family editor (VS Code, VSCodium, Cursor, …):

> **Extensions** view ▸ **⋯** menu ▸ **Install from VSIX…**

(or `code --install-extension <file>.vsix` / `codium --install-extension …` if
you prefer the CLI.)
