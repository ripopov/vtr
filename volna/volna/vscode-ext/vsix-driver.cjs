// Runs inside an isolated VS Code extension test host; vsix.test.mjs owns it.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const vscode = require("vscode");

exports.run = async function () {
  const root = process.env.VOLNA_VSIX_TEST_DIR;
  try {
    const extension = vscode.extensions.getExtension("vtr.volna");
    assert.ok(extension, "the VSIX is installed");
    // Observe the packaged relay's I/O without decoding trace data or
    // changing the real webview, server, or transport implementation.
    const bridge = require(path.join(extension.extensionPath, "trace-host.cjs"));
    const create = bridge.createTraceHost;
    const record = event => fs.appendFile(path.join(root, "trace-events.jsonl"), JSON.stringify(event) + "\n").catch(() => {});
    bridge.createTraceHost = (api, context, panel, uri) => {
      const sink = { webview: { postMessage: message => {
        record({ name: path.basename(uri.path), direction: "host", type: message.type, connection: message.connection, message: message.message });
        return panel.webview.postMessage(message);
      } } };
      const host = create(api, context, sink, uri);
      return { dispose: host.dispose, receive: message => {
        record({ name: path.basename(uri.path), direction: "viewer", type: message.type, connection: message.connection });
        return host.receive(message);
      } };
    };
    await extension.activate();
    for (const [index, name] of ["picorv32.vtr", "values.fst"].entries()) {
      const uri = vscode.Uri.file(path.join(root, name));
      await vscode.commands.executeCommand("vscode.open", uri, { preview: false });
      // Tab-group updates arrive asynchronously after the open command.
      const tabDeadline = Date.now() + 10000;
      let input;
      while (true) {
        input = vscode.window.tabGroups.activeTabGroup.activeTab?.input;
        if (input instanceof vscode.TabInputCustom && input.uri.toString() === uri.toString()) break;
        assert.ok(Date.now() < tabDeadline, `${name} opens a custom editor by default: ${JSON.stringify(input)}`);
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      assert.equal(input.viewType, "volna.waveform");
      await fs.writeFile(path.join(root, "current.json"), JSON.stringify({ index, name }));
      const deadline = Date.now() + 90000;
      while (true) {
        try { await fs.access(path.join(root, `passed-${index}`)); break; } catch {}
        assert.ok(Date.now() < deadline, `timed out verifying ${name}`);
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    }
    await fs.writeFile(path.join(root, "done"), "ok");
  } catch (error) {
    await fs.writeFile(path.join(root, "driver-error.txt"), error.stack);
    throw error;
  }
};
