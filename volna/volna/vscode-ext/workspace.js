// The extension host transports opaque workspace JSON. It never interprets a
// saved layout, timestamp, trace reference, or save-ticket counter.
const { randomUUID } = require("node:crypto");
const MAX_BYTES = 16 * 1024 * 1024;
const storageKey = (uri) => `volna.workspace:${uri.toString()}`;
const fileTarget = (uri) => ({ kind: "file", uri: uri.toString() });
const bytes = (value) => Array.from(value);

function createWorkspaceHost(vscode, context, panel, uri) {
  const webview = panel.webview;
  const sidecar = uri.with({ path: `${uri.path}.volna.json` });
  let latest;
  let disposed = false;
  let writes = Promise.resolve();
  const settings = () => {
    const config = vscode.workspace.getConfiguration("volna", uri);
    return { autosave: config.get("workspace.autosave", "sidecar"), linkByDefault: config.get("panels.linkByDefault", true),
      remote: { memoryMiB: config.get("remote.memoryMiB", 512), objectMiB: config.get("remote.objectMiB", 256) } };
  };
  const post = async (message) => {
    if (!disposed) {
      try { return await webview.postMessage(message); }
      catch (error) { if (!disposed) vscode.window.showErrorMessage(`Volna host message failed: ${error.message}`); }
    }
  };

  async function content(location) {
    try {
      const data = await vscode.workspace.fs.readFile(location);
      if (data.byteLength > MAX_BYTES) throw new Error("workspace exceeds size limit");
      return { status: "bytes", value: bytes(data) };
    } catch (error) {
      return error.code === "FileNotFound" ? { status: "missing" } : { status: "error", value: error.message };
    }
  }
  async function writable(location) {
    if (vscode.workspace.fs.isWritableFileSystem(location.scheme) === false) return false;
    // The scheme may support writes while this directory is read-only.
    const probe = location.with({ path: `${location.path}.probe-${randomUUID()}` });
    try {
      await vscode.workspace.fs.writeFile(probe, new Uint8Array());
      await vscode.workspace.fs.delete(probe, { useTrash: false });
      return true;
    } catch { return false; }
  }
  async function candidates(policy) {
    if (policy === "off") return undefined;
    const [sideContent, canWrite] = await Promise.all([content(sidecar), policy === "vscode" ? false : writable(sidecar)]);
    const stored = context.workspaceState.get(storageKey(uri));
    const fallbackContent = typeof stored === "string"
      ? Buffer.byteLength(stored, "utf8") <= MAX_BYTES
        ? { status: "bytes", value: bytes(Buffer.from(stored, "utf8")) }
        : { status: "error", value: "workspace exceeds size limit" }
      : stored === undefined ? { status: "missing" } : { status: "error", value: "invalid workspace storage value" };
    return {
      sidecar: { target: fileTarget(sidecar), content: sideContent, writable: canWrite },
      fallback: { target: { kind: "storage", key: storageKey(uri) }, content: fallbackContent, writable: true },
    };
  }
  async function ready() {
    const config = settings();
    const saved = await candidates(config.autosave);
    await post({ type: "open", traceUri: uri.toString(), name: uri.path.split("/").pop(),
      candidates: saved, settings: config });
  }
  function save(snapshot, acknowledge) {
    // Capture each message before queuing; never substitute a newer target or payload.
    const { ticket, json } = snapshot;
    writes = writes.then(async () => {
      let error;
      try {
        if (Buffer.byteLength(json, "utf8") > MAX_BYTES) throw new Error("workspace exceeds size limit");
        if (ticket.target.kind === "file") {
          await vscode.workspace.fs.writeFile(vscode.Uri.parse(ticket.target.uri), Buffer.from(json, "utf8"));
        } else if (ticket.target.kind === "storage") {
          await context.workspaceState.update(ticket.target.key, json);
        } else throw new Error("unsupported workspace destination");
      } catch (e) { error = e.message; }
      if (acknowledge) await post({ type: "saved", ticket, ...(error ? { error } : {}) });
      else if (error) vscode.window.showErrorMessage(`Volna workspace not saved: ${error}`);
    });
    return writes;
  }
  async function receive(message) {
    switch (message.type) {
      case "ready": return ready();
      case "workspace": latest = message; return save(message, true);
      case "saveWorkspaceAs": {
        const picked = await vscode.window.showSaveDialog({ defaultUri: sidecar, filters: { "Volna workspace": ["volna.json"] } });
        if (picked) return post({ type: "workspaceDestination", target: fileTarget(picked) });
        break;
      }
      case "openWorkspace": {
        const picked = await vscode.window.showOpenDialog({ canSelectMany: false, filters: { "Volna workspace": ["volna.json"] }, openLabel: "Open Workspace" });
        if (picked?.[0]) return post({ type: "workspace", candidate: { target: fileTarget(picked[0]), content: await content(picked[0]), writable: true } });
        break;
      }
      case "closeTrace": {
        if (message.traceUri !== uri.toString()) break;
        const tab = vscode.window.tabGroups.all.flatMap((group) => group.tabs)
          .find((tab) => tab.input?.uri?.toString() === uri.toString() && tab.input?.viewType === "volna.waveform");
        if (tab) await vscode.window.tabGroups.close(tab);
        break;
      }
      case "notice": vscode.window.showWarningMessage(`Volna: ${message.text}`); break;
    }
  }
  function hidden() { return post({ type: "requestWorkspace" }); }
  function dispose() {
    disposed = true;
    // The webview is already gone. Only use the snapshot received while alive.
    return latest ? save(latest, false) : writes;
  }
  return { receive, hidden, dispose, settled: () => writes };
}
module.exports = { createWorkspaceHost, storageKey };
