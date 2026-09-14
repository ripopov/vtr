const test = require("node:test");
const assert = require("node:assert/strict");
const { createWorkspaceHost, storageKey } = require("./workspace");

class Uri {
  constructor(value) { this.url = new URL(value); }
  get path() { return decodeURI(this.url.pathname); }
  get scheme() { return this.url.protocol.slice(0, -1); }
  with({ path }) { const uri = new Uri(this.toString()); uri.url.pathname = path; return uri; }
  toString() { return this.url.toString(); }
  static parse(value) { return new Uri(value); }
}
function fixture(autosave = "sidecar", options) {
  const uri = new Uri("vscode-remote://ssh-remote+board/home/user/trace.vtr");
  const sidecar = `${uri}.volna.json`;
  const files = new Map([[uri.toString(), Buffer.from([1, 2, 3])]]);
  const storage = new Map();
  const posted = [], written = [], read = [], notices = [];
  let failWrite, delayWrite;
  const vscode = {
    Uri,
    workspace: {
      getConfiguration: () => ({ get: (key, fallback) => key === "workspace.autosave" ? autosave : fallback }),
      fs: {
        isWritableFileSystem: () => true,
        async readFile(uri) { read.push(uri.toString()); if (!files.has(uri.toString())) throw Object.assign(new Error("not found"), { code: "FileNotFound" }); return files.get(uri.toString()); },
        async writeFile(uri, bytes) {
          if (failWrite) throw new Error(failWrite);
          if (delayWrite) await delayWrite;
          written.push([uri.toString(), Buffer.from(bytes)]); files.set(uri.toString(), Buffer.from(bytes));
        },
        async delete(uri) { files.delete(uri.toString()); },
      },
    },
    window: { showWarningMessage: (s) => notices.push(s), showErrorMessage: (s) => notices.push(s), showSaveDialog: async () => undefined, showOpenDialog: async () => undefined },
  };
  const context = { workspaceState: { get: (key) => storage.get(key), update: async (key, value) => storage.set(key, value) } };
  const panel = { webview: { postMessage: async (message) => { posted.push(message); return true; } } };
  const host = createWorkspaceHost(vscode, context, panel, uri, options);
  return { host, uri, sidecar, files, storage, posted, written, read, notices, vscode, fail: (error) => failWrite = error, delay: (promise) => delayWrite = promise };
}
const ticket = (target, revision = "18446744073709551614") => ({ target, epoch: "18446744073709551613", revision });
const snapshot = (target, json, revision) => ({ type: "workspace", ticket: ticket(target, revision), json });

test("open forwards both candidates unchanged and preserves remote resource identity", async () => {
  const f = fixture();
  const json = '{ "cursor":18446744073709551614, "unknown":{"a": 1} }';
  const fallback = '{"supersedes":"opaque-to-host","cursor":9007199254740993}';
  f.files.set(f.sidecar, Buffer.from(json));
  f.storage.set(storageKey(f.uri), fallback);
  await f.host.receive({ type: "ready" });
  const open = f.posted[0];
  assert.equal(open.traceUri, f.uri.toString());
  assert.equal(open.candidates.sidecar.target.uri, f.sidecar);
  assert.equal(Buffer.from(open.candidates.sidecar.content.value).toString(), json);
  assert.equal(Buffer.from(open.candidates.fallback.content.value).toString(), fallback);
  assert.deepEqual(Array.from(new Uint8Array(open.bytes)), [1, 2, 3]);
  assert.equal(open.candidates.sidecar.writable, true);
  assert.equal([...f.files.keys()].filter((k) => k.includes(".probe-")).length, 0);
});

test("disabled persistence reads only trace bytes and never probes or writes", async () => {
  const f = fixture("off");
  await f.host.receive({ type: "ready" });
  assert.equal(f.posted[0].candidates, undefined);
  assert.deepEqual(f.read, [f.uri.toString()]);
  assert.equal(f.written.length, 0);
  await f.host.dispose();
  assert.equal(f.written.length, 0);
});

test("writes follow each ticket destination and echo exact string counters", async () => {
  const f = fixture();
  const json = '{ "cursor":18446744073709551614 }';
  const other = "vscode-remote://ssh-remote+board/other/saved.volna.json";
  const message = snapshot({ kind: "file", uri: other }, json);
  await f.host.receive(message);
  assert.equal(f.files.get(other).toString(), json);
  assert.deepEqual(f.posted[0], { type: "saved", ticket: message.ticket });
  const stored = snapshot({ kind: "storage", key: "chosen-key" }, "not parsed by host");
  await f.host.receive(stored);
  assert.equal(f.storage.get("chosen-key"), stored.json);
  assert.deepEqual(f.posted[1].ticket, stored.ticket);
});

test("errors acknowledge the same ticket and subsequent writes continue", async () => {
  const f = fixture();
  const first = snapshot({ kind: "file", uri: f.sidecar }, "first", "1");
  f.fail("permission denied");
  await f.host.receive(first);
  assert.deepEqual(f.posted[0], { type: "saved", ticket: first.ticket, error: "permission denied" });
  f.fail(undefined);
  await f.host.receive(snapshot(first.ticket.target, "second", "2"));
  assert.equal(f.files.get(f.sidecar).toString(), "second");
});

test("hide requests a snapshot; disposal serializes cached bytes without messaging a dead webview", async () => {
  const f = fixture();
  await f.host.hidden();
  assert.deepEqual(f.posted, [{ type: "requestWorkspace" }]);
  let release;
  f.delay(new Promise((resolve) => release = resolve));
  const first = f.host.receive(snapshot({ kind: "file", uri: f.sidecar }, "first", "1"));
  const last = f.host.receive(snapshot({ kind: "storage", key: "fallback" }, "last", "2"));
  const disposed = f.host.dispose();
  release();
  await Promise.all([first, last, disposed]);
  assert.equal(f.files.get(f.sidecar).toString(), "first");
  assert.equal(f.storage.get("fallback"), "last");
  assert.deepEqual(f.posted, [{ type: "requestWorkspace" }]);
});

test("unreadable sidecar differs from absence and unwritable directories are reported", async () => {
  const f = fixture();
  f.vscode.workspace.fs.readFile = async (uri) => {
    if (uri.toString() === f.uri.toString()) return Buffer.from([1]);
    throw Object.assign(new Error("denied"), { code: "NoPermissions" });
  };
  f.fail("directory denied");
  await f.host.receive({ type: "ready" });
  assert.deepEqual(f.posted[0].candidates.sidecar.content, { status: "error", value: "denied" });
  assert.equal(f.posted[0].candidates.sidecar.writable, false);
});

test("closing a trace closes its own custom editor even when another tab is active", async () => {
  const f = fixture();
  const owner = { input: { uri: f.uri, viewType: "volna.waveform" } };
  const other = { input: { uri: new Uri("file:///other.txt") }, isActive: true };
  const closed = [];
  f.vscode.window.tabGroups = { all: [{ tabs: [other, owner] }], close: async (tab) => closed.push(tab) };
  await f.host.receive({ type: "closeTrace", traceUri: f.uri.toString() });
  assert.deepEqual(closed, [owner]);
  await f.host.receive({ type: "closeTrace", traceUri: "file:///other.txt" });
  assert.deepEqual(closed, [owner]);
});

test("query opening forwards workspace metadata without reading trace bytes", async () => {
  const opened = [];
  const f = fixture("sidecar", { openQuery: async metadata => opened.push(metadata) });
  await f.host.receive({ type: "ready", transport: "rpc" });
  assert.equal(opened.length, 1);
  assert.equal(opened[0].traceUri, f.uri.toString());
  assert.equal(opened[0].name, "trace.vtr");
  assert.equal(opened[0].candidates.sidecar.target.uri, f.sidecar);
  assert.equal("bytes" in opened[0], false);
  assert.deepEqual(f.read, [f.sidecar]);
  assert.equal(f.posted.length, 0);
});

test("query opening with persistence off performs no filesystem reads", async () => {
  const opened = [];
  const f = fixture("off", { openQuery: async metadata => opened.push(metadata) });
  await f.host.receive({ type: "ready", transport: "rpc" });
  assert.equal(opened.length, 1);
  assert.deepEqual(f.read, []);
  assert.deepEqual(f.written, []);
});
