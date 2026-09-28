// The manifest's commands and keys match what the extension forwards, and
// the forwarded commands reach the active viewer (docs/undo-redo.html).
const test = require("node:test");
const assert = require("node:assert/strict");
const Module = require("node:module");
const manifest = require("./package.json");

const WHEN = "activeCustomEditorId == volna.waveform && !inputFocus";

/** Load extension.js against a fake `vscode` and activate it. */
function activated() {
  const commands = new Map();
  let provider;
  const disposable = { dispose() {} };
  const vscode = {
    Uri: { joinPath: (base, ...parts) => ({ path: [base.path, ...parts].join("/") }) },
    commands: {
      registerCommand: (name, callback) => { commands.set(name, callback); return disposable; },
      executeCommand: async () => {},
    },
    window: {
      registerCustomEditorProvider: (_type, value) => { provider = value; return disposable; },
      showErrorMessage() {},
    },
    workspace: { onDidChangeConfiguration: () => disposable },
  };
  const load = Module._load;
  Module._load = function (request, ...rest) {
    return request === "vscode" ? vscode : load.call(this, request, ...rest);
  };
  let extension;
  try {
    delete require.cache[require.resolve("./extension.js")];
    extension = require("./extension.js");
  } finally {
    Module._load = load;
  }
  extension.activate({ subscriptions: [], extensionUri: { path: "/extension" } });
  return { commands, provider, extension };
}

function panel(active) {
  const posted = [];
  const disposable = { dispose() {} };
  return {
    active,
    visible: true,
    posted,
    onDidChangeViewState: () => disposable,
    onDidDispose: () => disposable,
    webview: {
      cspSource: "vscode-webview:",
      asWebviewUri: (uri) => uri.path,
      onDidReceiveMessage: () => disposable,
      postMessage: async (message) => { posted.push(message); return true; },
    },
  };
}

test("undo and redo are contributed with the platform keys, only over a Volna editor", () => {
  const titles = new Map(manifest.contributes.commands.map((c) => [c.command, c.title]));
  assert.equal(titles.get("volna.undo"), "Volna: Undo");
  assert.equal(titles.get("volna.redo"), "Volna: Redo");
  const keys = manifest.contributes.keybindings
    .filter((k) => ["volna.undo", "volna.redo"].includes(k.command))
    .map((k) => [k.command, k.key, k.mac, k.when]);
  assert.deepEqual(keys, [
    ["volna.undo", "ctrl+z", "cmd+z", WHEN],
    ["volna.redo", "ctrl+shift+z", "cmd+shift+z", WHEN],
    ["volna.redo", "ctrl+y", "cmd+shift+z", WHEN],
  ]);
  for (const binding of manifest.contributes.keybindings) {
    assert.equal(binding.when, WHEN, `${binding.command} is scoped to the viewer`);
  }
});

test("every contributed viewer command is forwarded and every forwarded one contributed", () => {
  const { commands, extension } = activated();
  const contributed = manifest.contributes.commands
    .map((c) => c.command)
    .filter((c) => !["volna.open", "volna.openSettings"].includes(c));
  assert.deepEqual(
    contributed.slice().sort(),
    extension.FORWARDED_COMMANDS.map((name) => `volna.${name}`).sort(),
  );
  for (const command of contributed) assert.ok(commands.has(command), command);
});

test("volna.undo and volna.redo post their names to the active viewer only", async () => {
  const { commands, provider } = activated();
  const background = panel(false);
  const active = panel(true);
  const uri = { path: "/trace.vtr", toString: () => "file:///trace.vtr", with: () => uri };
  await provider.resolveCustomEditor({ uri }, background);
  await provider.resolveCustomEditor({ uri }, active);
  await commands.get("volna.undo")();
  await commands.get("volna.redo")();
  assert.deepEqual(active.posted, [
    { type: "command", name: "undo" },
    { type: "command", name: "redo" },
  ]);
  assert.deepEqual(background.posted, []);
});
