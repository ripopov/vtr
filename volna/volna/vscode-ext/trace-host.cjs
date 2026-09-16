// The workspace extension owns one child beside its authorized trace URI.
// Binary frames remain opaque here; Rust owns commands, IDs and data semantics.
const { spawn } = require("node:child_process");
const path = require("node:path");

const HEADER = 12;
const MAX_FRAME = 1024 * 1024;

class FrameReader {
  constructor(deliver) {
    this.deliver = deliver;
    this.header = Buffer.alloc(HEADER);
    this.used = 0;
    this.frame = undefined;
  }
  push(bytes) {
    let offset = 0;
    while (offset < bytes.length) {
      const target = this.frame ?? this.header;
      const count = Math.min(target.length - this.used, bytes.length - offset);
      bytes.copy(target, this.used, offset, offset + count);
      offset += count;
      this.used += count;
      if (this.used !== target.length) continue;
      if (!this.frame) {
        if (this.header.toString("ascii", 0, 4) !== "VLNA") throw new Error("invalid server frame magic");
        const length = this.header.readUInt32LE(8);
        if (!length || length > MAX_FRAME) throw new Error("server frame exceeds size limit");
        this.frame = Buffer.alloc(HEADER + length);
        this.header.copy(this.frame);
      } else {
        const frame = this.frame;
        this.frame = undefined;
        this.used = 0;
        this.deliver(frame);
      }
    }
  }
  end() {
    if (this.used) throw new Error("server ended inside a frame");
  }
}

function packetBytes(value) {
  let bytes;
  if (value instanceof ArrayBuffer) bytes = Buffer.from(value);
  else if (ArrayBuffer.isView(value)) bytes = Buffer.from(value.buffer, value.byteOffset, value.byteLength);
  else if (Array.isArray(value) && value.length <= HEADER + MAX_FRAME && value.every(v => Number.isInteger(v) && v >= 0 && v <= 255)) bytes = Buffer.from(value);
  else throw new Error("invalid client frame bytes");
  if (bytes.length < HEADER || bytes.length > HEADER + MAX_FRAME || bytes.toString("ascii", 0, 4) !== "VLNA" || bytes.readUInt32LE(8) !== bytes.length - HEADER) throw new Error("invalid client frame length");
  return bytes;
}

function createTraceHost(vscode, context, panel, uri, spawnChild = spawn) {
  let active;
  let disposed = false;
  let epoch = 0;

  function stop() {
    epoch += 1;
    const previous = active;
    active = undefined;
    if (previous) {
      previous.child.stdin.destroy();
      previous.child.kill();
    }
  }
  function post(message) {
    if (!disposed) return panel.webview.postMessage(message);
    return Promise.resolve(false);
  }
  function fail(state, message) {
    if (disposed || active !== state) return;
    const connection = state.connection;
    stop();
    Promise.resolve(post({ type: "traceError", connection, message })).catch(() => {});
  }

  async function start(connection) {
    if (disposed) throw new Error("trace host is closed");
    if (typeof connection !== "string" || !connection.length || connection.length > 128) throw new Error("invalid trace connection identity");
    // file: is used in local extension hosts; vscode-remote: maps to a native
    // fsPath in workspace extension hosts. Other virtual files need a loader.
    if (!["file", "vscode-remote"].includes(uri.scheme)) throw new Error(`Remote trace loading does not support ${uri.scheme}: resources`);
    stop();
    const ownEpoch = epoch;
    const configured = vscode.workspace.getConfiguration("volna", uri).get("serverPath", "");
    const executable = configured || path.join(context.extensionPath, "bin", process.platform === "win32" ? "volna-server.exe" : "volna-server");
    const child = spawnChild(executable, [uri.fsPath], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true, shell: false });
    const state = { child, connection, awaitingClient: false, pendingWrites: 0, writes: Promise.resolve(), diagnostics: "" };
    active = state;
    const reader = new FrameReader(frame => {
      if (disposed || active !== state || epoch !== ownEpoch) return;
      if (state.awaitingClient) throw new Error("server sent a frame before client acknowledgement");
      state.awaitingClient = true;
      // A dedicated ArrayBuffer avoids sending a pooled Buffer's unrelated bytes.
      const bytes = frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength);
      Promise.resolve(post({ type: "traceFrame", connection, bytes })).then(delivered => {
        if (!delivered) fail(state, "viewer could not receive a trace frame");
      }, error => fail(state, `viewer delivery failed: ${error.message}`));
    });
    child.stdout.on("data", bytes => {
      if (active !== state) return;
      try { reader.push(bytes); } catch (error) { fail(state, error.message); }
    });
    child.stderr.on("data", bytes => {
      state.diagnostics = (state.diagnostics + bytes.toString("utf8")).slice(-4096);
    });
    child.stdin.on("error", error => fail(state, `server input failed: ${error.message}`));
    child.stdout.on("error", error => fail(state, `server output failed: ${error.message}`));
    child.on("error", error => fail(state, `Cannot start Volna server: ${error.message}`));
    child.on("exit", (code, signal) => {
      let detail = state.diagnostics.trim();
      try { reader.end(); } catch (error) { detail = error.message; }
      fail(state, detail || `Volna server disconnected (${signal || code})`);
    });
  }

  async function receive(message) {
    if (message.type === "traceStart") return start(message.connection);
    if (message.type === "traceStop") {
      if (active?.connection === message.connection) stop();
      return;
    }
    if (message.type !== "traceRequest") return;
    const state = active;
    if (!state || state.connection !== message.connection || disposed) return;
    let bytes;
    try {
      bytes = packetBytes(message.bytes);
      if (state.pendingWrites >= 2) throw new Error("trace request queue is full");
    } catch (error) { fail(state, error.message); return; }
    state.pendingWrites += 1;
    state.writes = state.writes.then(() => new Promise((resolve, reject) => {
      if (active !== state) { resolve(); return; }
      state.awaitingClient = false;
      state.child.stdin.write(bytes, error => error ? reject(error) : resolve());
    })).catch(error => fail(state, `server write failed: ${error.message}`)).finally(() => { state.pendingWrites -= 1; });
    return state.writes;
  }

  function dispose() { disposed = true; stop(); }
  return { receive, dispose };
}

module.exports = { createTraceHost, FrameReader, packetBytes };
