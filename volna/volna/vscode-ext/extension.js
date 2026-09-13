// Volna VS Code extension: hosts the wasm build of the viewer in a webview.
//
// Two entry points:
//  - a read-only custom editor for *.vtr and *.fst files (bytes are sent to the
//    webview, which hands them to the wasm module), and
//  - the "Volna: Open Waveform Viewer" command, which picks a custom editor input.
//
// The webview never touches the file system: the extension host reads files
// via vscode.workspace.fs and posts the bytes over postMessage.
const vscode = require("vscode");
const { createWorkspaceHost } = require("./workspace");

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
  import init, { open_resource, set_vscode_theme, dispatch_command, workspace_message } from "${js}";
  import { watchTheme } from "${themeJs}";
  const vscode = acquireVsCodeApi();
  window.volnaEmbedded = true;
  window.volnaOpen = () => vscode.postMessage({ type: "pickFile" });
  window.volnaReady = () => vscode.postMessage({ type: "ready" });
  window.volnaWorkspace = (envelope) => vscode.postMessage(JSON.parse(envelope));
  window.addEventListener("message", (ev) => {
    const msg = ev.data;
    if (msg && msg.type === "open") {
      open_resource(msg.name, new Uint8Array(msg.bytes), JSON.stringify({ traceUri: msg.traceUri, candidates: msg.candidates, settings: msg.settings }));
    } else if (msg && msg.type === "command") {
      dispatch_command(msg.name);
    } else if (msg && ["saved", "workspace", "workspaceDestination", "requestWorkspace"].includes(msg.type)) {
      workspace_message(JSON.stringify(msg));
    }
  });
  const themes = watchTheme(set_vscode_theme);
  window.volnaVscodeTheme = themes.current;
  await init();
  themes.start();
</script>
</body>
</html>`;
}

async function pickTrace() {
  const picked = await vscode.window.showOpenDialog({
    canSelectMany: false,
    filters: { "Waveform traces": ["vtr", "fst"] },
    openLabel: "Open",
  });
  if (picked?.[0]) await vscode.commands.executeCommand("vscode.openWith", picked[0], "volna.waveform");
}

function wire(panel, context, uri) {
  const webview = panel.webview;
  const host = createWorkspaceHost(vscode, context, panel, uri);
  panel.onDidChangeViewState(() => { if (!panel.visible) host.hidden(); });
  panel.onDidDispose(() => host.dispose());
  webview.options = { enableScripts: true, localResourceRoots: [vscode.Uri.joinPath(context.extensionUri, "media")] };
  webview.onDidReceiveMessage(async (msg) => {
    try {
      if (msg.type === "pickFile") await pickTrace();
      else await host.receive(msg);
    } catch (error) {
      vscode.window.showErrorMessage(`Volna: ${error.message}`);
    }
  });
  // Register the receiver before loading HTML: ready may arrive immediately.
  webview.html = html(webview, context.extensionUri);
}

function activate(context) {
  let activePanel;
  const commandNames = [
    "splitRight", "splitDown", "newPanel", "closePanel",
    "focusNextPanel", "focusPrevPanel", "toggleViewportLink", "toggleCursorLink",
    "openWorkspace", "saveWorkspace", "saveWorkspaceAs",
    ...Array.from({ length: 9 }, (_, i) => `focusPanel${i + 1}`),
  ];
  context.subscriptions.push(vscode.commands.registerCommand("volna.open", pickTrace));
  for (const name of commandNames) {
    context.subscriptions.push(vscode.commands.registerCommand(`volna.${name}`, () =>
      activePanel?.webview.postMessage({ type: "command", name })));
  }
  context.subscriptions.push(
    vscode.window.registerCustomEditorProvider(
      "volna.waveform",
      {
        async openCustomDocument(uri) {
          return { uri, dispose() {} };
        },
        async resolveCustomEditor(document, panel) {
          if (panel.active) activePanel = panel;
          panel.onDidChangeViewState(() => {
            if (panel.active) activePanel = panel;
            else if (activePanel === panel) activePanel = undefined;
          });
          panel.onDidDispose(() => {
            if (activePanel === panel) activePanel = undefined;
          });
          wire(panel, context, document.uri);
        },
      },
      { webviewOptions: { retainContextWhenHidden: true }, supportsMultipleEditorsPerDocument: false }
    )
  );
}

function deactivate() {}

module.exports = { activate, deactivate };
