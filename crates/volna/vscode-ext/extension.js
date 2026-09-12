// Volna VS Code extension: hosts the wasm build of the viewer in a webview.
//
// Two entry points:
//  - a read-only custom editor for *.vtr files (the file bytes are sent to the
//    webview, which hands them to the wasm module), and
//  - the "Volna: Open Waveform Viewer" command, which opens an empty viewer.
//
// The webview never touches the file system: the extension host reads files
// via vscode.workspace.fs and posts the bytes over postMessage.
const vscode = require("vscode");

function nonce() {
  let s = "";
  const a = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  for (let i = 0; i < 32; i++) s += a[Math.floor(Math.random() * a.length)];
  return s;
}

function html(webview, extensionUri) {
  const media = vscode.Uri.joinPath(extensionUri, "media");
  const js = webview.asWebviewUri(vscode.Uri.joinPath(media, "volna.js"));
  const themeJs = webview.asWebviewUri(vscode.Uri.joinPath(media, "theme.mjs"));
  const n = nonce();
  const csp = [
    `default-src 'none'`,
    `img-src ${webview.cspSource} blob: data:`,
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `script-src 'nonce-${n}' 'wasm-unsafe-eval' ${webview.cspSource}`,
    `connect-src ${webview.cspSource}`,
    `font-src ${webview.cspSource}`,
  ].join("; ");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta http-equiv="Content-Security-Policy" content="${csp}" />
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { height: 100%; background: var(--vscode-editor-background); color: var(--vscode-editor-foreground); overflow: hidden; }
  canvas { display: block; width: 100%; height: 100%; touch-action: none; outline: none; user-select: none; }
</style>
</head>
<body>
<script type="module" nonce="${n}">
  import init, { open_trace, start, set_theme } from "${js}";
  import { followTheme } from "${themeJs}";
  const vscode = acquireVsCodeApi();
  window.volnaEmbedded = true;
  window.volnaOpen = () => vscode.postMessage({ type: "pickFile" });
  window.volnaReady = () => vscode.postMessage({ type: "ready" });
  window.addEventListener("message", (ev) => {
    const msg = ev.data;
    if (msg && msg.type === "open") {
      open_trace(msg.name, new Uint8Array(msg.bytes));
    }
  });
  await init();
  await followTheme(set_theme);
  start();
</script>
</body>
</html>`;
}

async function sendFile(webview, uri) {
  const bytes = await vscode.workspace.fs.readFile(uri);
  const name = uri.path.split("/").pop();
  webview.postMessage({ type: "open", name, bytes: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
}

function wire(panelWebview, context, initialUri) {
  panelWebview.options = { enableScripts: true, localResourceRoots: [vscode.Uri.joinPath(context.extensionUri, "media")] };
  let current = initialUri;
  panelWebview.onDidReceiveMessage(async (msg) => {
    if (msg.type === "ready" && current) {
      await sendFile(panelWebview, current);
    } else if (msg.type === "pickFile") {
      const picked = await vscode.window.showOpenDialog({
        canSelectMany: false,
        filters: { "VTR traces": ["vtr"] },
        openLabel: "Open",
      });
      if (picked && picked[0]) {
        current = picked[0];
        await sendFile(panelWebview, current);
      }
    }
  });
  // Register the receiver before loading HTML: ready may arrive immediately.
  panelWebview.html = html(panelWebview, context.extensionUri);
}

function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand("volna.open", () => {
      const panel = vscode.window.createWebviewPanel("volna.viewer", "Volna", vscode.ViewColumn.Active, {
        enableScripts: true,
        retainContextWhenHidden: true,
      });
      wire(panel.webview, context, undefined);
    })
  );
  context.subscriptions.push(
    vscode.window.registerCustomEditorProvider(
      "volna.waveform",
      {
        async openCustomDocument(uri) {
          return { uri, dispose() {} };
        },
        async resolveCustomEditor(document, panel) {
          wire(panel.webview, context, document.uri);
        },
      },
      { webviewOptions: { retainContextWhenHidden: true }, supportsMultipleEditorsPerDocument: true }
    )
  );
}

function deactivate() {}

module.exports = { activate, deactivate };
