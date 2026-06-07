// VSCodium/VS Code extension: a theme-aware custom editor for .dif files.
//
// The file bytes are decoded inside a webview by the dif-wasm module. When the
// editor color theme changes, the extension posts the new kind and the webview
// re-renders the matching DIF theme.
import * as vscode from "vscode";

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(DifEditorProvider.register(context));
}

export function deactivate(): void {
  /* nothing to clean up */
}

function themeKindString(kind: vscode.ColorThemeKind): string {
  switch (kind) {
    case vscode.ColorThemeKind.Dark:
      return "dark";
    case vscode.ColorThemeKind.HighContrast:
      return "high-contrast";
    case vscode.ColorThemeKind.HighContrastLight:
      return "high-contrast";
    default:
      return "light";
  }
}

function nonce(): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let s = "";
  for (let i = 0; i < 32; i++) s += chars.charAt(Math.floor(Math.random() * chars.length));
  return s;
}

class DifEditorProvider implements vscode.CustomReadonlyEditorProvider {
  public static register(context: vscode.ExtensionContext): vscode.Disposable {
    return vscode.window.registerCustomEditorProvider("dif.preview", new DifEditorProvider(context), {
      webviewOptions: { retainContextWhenHidden: true },
      supportsMultipleEditorsPerDocument: true,
    });
  }

  constructor(private readonly context: vscode.ExtensionContext) {}

  public openCustomDocument(uri: vscode.Uri): vscode.CustomDocument {
    return { uri, dispose: () => undefined };
  }

  public async resolveCustomEditor(
    document: vscode.CustomDocument,
    panel: vscode.WebviewPanel
  ): Promise<void> {
    const mediaRoot = vscode.Uri.joinPath(this.context.extensionUri, "media");
    panel.webview.options = { enableScripts: true, localResourceRoots: [mediaRoot] };

    const bytes = await vscode.workspace.fs.readFile(document.uri);
    const b64 = Buffer.from(bytes).toString("base64");
    panel.webview.html = this.html(panel.webview, mediaRoot, b64);

    const postTheme = () =>
      panel.webview.postMessage({ type: "theme", kind: themeKindString(vscode.window.activeColorTheme.kind) });
    const sub = vscode.window.onDidChangeActiveColorTheme(postTheme);
    panel.onDidDispose(() => sub.dispose());

    // The webview asks for the theme once it has booted the wasm module.
    panel.webview.onDidReceiveMessage((msg) => {
      if (msg?.type === "ready") postTheme();
    });
  }

  private html(webview: vscode.Webview, mediaRoot: vscode.Uri, b64: string): string {
    const uri = (...p: string[]) => webview.asWebviewUri(vscode.Uri.joinPath(mediaRoot, ...p));
    const pkgJs = uri("pkg", "dif_wasm.js");
    const wasm = uri("pkg", "dif_wasm_bg.wasm");
    const viewer = uri("viewer.js");
    // The decoder is built for wasm32-wasip1 (so the C codecs zstd/lzav get
    // wasi-libc's malloc); its JS glue imports `wasi_snapshot_preview1`. An
    // import map resolves that bare specifier to a no-op shim --- the decode path
    // does no I/O, so nothing in it runs (mirrors web/index.html).
    const wasiShim = uri("wasi_shim.js");
    const n = nonce();
    // Module imports (the dynamic `import(pkg)` + the import-map shim) load from
    // the webview origin, so script-src must allow `cspSource` alongside the
    // nonce that covers the inline scripts.
    const csp =
      `default-src 'none'; img-src ${webview.cspSource}; style-src ${webview.cspSource} 'unsafe-inline'; ` +
      `script-src 'nonce-${n}' 'wasm-unsafe-eval' ${webview.cspSource}; connect-src ${webview.cspSource};`;
    return `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta http-equiv="Content-Security-Policy" content="${csp}" />
    <style>
      body { margin: 0; display: grid; place-items: center; height: 100vh; background: var(--vscode-editor-background); }
      canvas { max-width: 100%; max-height: 100%; image-rendering: pixelated; }
      #err { color: var(--vscode-errorForeground); font-family: var(--vscode-editor-font-family); white-space: pre-wrap; }
    </style>
  </head>
  <body>
    <canvas id="view"></canvas>
    <pre id="err"></pre>
    <script nonce="${n}">
      window.__DIF = { b64: "${b64}", pkg: "${pkgJs}", wasm: "${wasm}" };
    </script>
    <script type="importmap" nonce="${n}">
      { "imports": { "wasi_snapshot_preview1": "${wasiShim}" } }
    </script>
    <script nonce="${n}" type="module" src="${viewer}"></script>
  </body>
</html>`;
  }
}
