// Opaque framed-byte relay. Query IDs, timestamps and Protobuf fields stay in
// Rust. Courier tokens below identify transport acknowledgements only.
const { spawn: spawnProcess } = require("node:child_process");
const { randomBytes } = require("node:crypto");
const path = require("node:path");

const LIMITS = Object.freeze({
  request: 64 * 1024, reply: 1024 * 1024,
  replySlots: 12, replyBytes: 4 * 1024 * 1024 + 8 * 16 * 1024,
  readChunk: 16 * 1024, readTurn: 64 * 1024, diagnostics: 8192,
});
function serverBinary(extensionPath, platform = process.platform, arch = process.arch) {
  if (!["darwin", "linux", "win32"].includes(platform) || !["arm64", "x64"].includes(arch)) {
    throw new Error(`Unsupported query host: ${platform}-${arch}`);
  }
  return path.join(extensionPath, "bin", `${platform}-${arch}`, platform === "win32" ? "vtr-server.exe" : "vtr-server");
}

class Relay {
  constructor({ binary, tracePath, postMessage, spawn = spawnProcess,
    incarnation = randomBytes(16).toString("hex"), visible = true,
    graceMs = 500, killMs = 1500, onError = () => {} }) {
    if (!path.isAbsolute(binary) || !path.isAbsolute(tracePath)) throw new Error("Query child paths must be absolute");
    if (!/^[0-9a-f]{32}$/.test(incarnation)) throw new Error("Invalid relay incarnation");
    this.incarnation = incarnation;
    this.postMessage = postMessage;
    this.onError = onError;
    this.visible = visible;
    this.graceMs = graceMs;
    this.killMs = killMs;
    this.accepting = true;
    this.disposed = false;
    this.childClosed = false;
    this.exitPending = undefined;
    this.failure = undefined;
    this.pending = new Map();
    this.reservedBytes = 0;
    this.posts = 0;
    this.sendSequence = 0n;
    this.replySequence = 0n;
    this.sending = undefined;
    this.prefix = Buffer.alloc(4);
    this.prefixUsed = 0;
    this.length = undefined;
    this.payload = undefined;
    this.offset = 0;
    this.scheduled = false;
    this.diagnostic = Buffer.alloc(0);
    this.timers = [];
    this.stopped = new Promise(resolve => { this.resolveStopped = resolve; });
    // No shell, cwd from the trace, network listener, or script-supplied path.
    this.child = spawn(binary, ["--stdio", tracePath], {
      shell: false, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
    });
    this.child.on("error", error => this.fail(error.message));
    this.child.on("close", (code, signal) => this.closed(code, signal));
    this.child.stdin.on("error", error => { if (!this.disposed) this.fail(`Query input: ${error.message}`); });
    this.child.stdout.on("error", error => this.fail(`Query output: ${error.message}`));
    this.child.stdout.on("readable", () => this.schedule());
    this.child.stdout.on("end", () => {
      if (!this.disposed && (this.prefixUsed || this.length !== undefined)) this.fail("Truncated query frame");
    });
    this.child.stderr.on("data", bytes => {
      const tail = bytes.subarray(Math.max(0, bytes.length - LIMITS.diagnostics));
      this.diagnostic = Buffer.concat([this.diagnostic.subarray(Math.max(0,
        this.diagnostic.length + tail.length - LIMITS.diagnostics)), tail]);
    });
    this.child.stderr.on("error", () => {});
  }

  // A sender must wait for rpcWritten before asking RpcDriver for another
  // outbound packet. Controls remain in Rust's priority queue until that point.
  receive(message) {
    if (!message || message.incarnation !== this.incarnation || !this.accepting) return false;
    try {
      if (message.type === "rpcSend") {
        if (this.sending) throw new Error("Query packet sent without relay credit");
        const token = (this.sendSequence + 1n).toString();
        if (message.token !== token) throw new Error("Invalid query send token");
        if (!(message.bytes instanceof ArrayBuffer) || message.bytes.byteLength === 0 || message.bytes.byteLength > LIMITS.request) {
          throw new Error("Invalid query packet size or representation");
        }
        this.sendSequence++;
        const payload = Buffer.from(message.bytes);
        const header = Buffer.alloc(4);
        header.writeUInt32LE(payload.length);
        this.sending = { token, bytes: payload.length };
        this.child.stdin.cork();
        try {
          this.child.stdin.write(header);
          this.child.stdin.write(payload, error => {
            this.sending = undefined;
            if (error && !this.disposed) this.fail(`Query write: ${error.message}`);
            else if (!this.disposed && this.accepting) this.post({ type: "rpcWritten", token });
            this.finish();
          });
        } finally { this.child.stdin.uncork(); }
      } else if (message.type === "rpcConsumed") {
        if (typeof message.token !== "string" || !/^[1-9][0-9]{0,19}$/.test(message.token)
          || BigInt(message.token) > this.replySequence) throw new Error("Invalid query delivery acknowledgement");
        const record = this.pending.get(message.token);
        if (record) {
          record.consumed = true;
          this.release(message.token, record);
        }
      } else return false;
      return true;
    } catch (error) { this.fail(error.message); return false; }
  }

  post(message, record, token) {
    this.posts++;
    let result;
    try { result = this.postMessage({ ...message, incarnation: this.incarnation }); }
    catch (error) { result = Promise.reject(error); }
    Promise.resolve(result).then(posted => {
      if (record) {
        record.posted = true;
        if (this.disposed || this.childClosed) record.consumed = true;
        this.release(token, record);
      }
      if (posted !== true && !this.disposed) this.fail("Webview did not accept query transport message");
    }, error => {
      if (record) { record.posted = true; record.consumed = true; this.release(token, record); }
      if (!this.disposed) this.fail(`Webview query transport: ${error.message}`);
    }).finally(() => { this.posts--; this.finish(); });
  }
  release(token, record) {
    if (record.posted && record.consumed && this.pending.get(token) === record) {
      this.pending.delete(token);
      this.reservedBytes -= record.bytes;
      this.schedule();
    }
  }
  schedule() {
    if (this.scheduled || !this.accepting || !this.visible) return;
    this.scheduled = true;
    setImmediate(() => {
      this.scheduled = false;
      try { this.pump(); } catch (error) { this.fail(error.message); }
    });
  }
  pump() {
    if (!this.accepting || !this.visible) return;
    let copied = 0;
    while (copied < LIMITS.readTurn) {
      if (this.length === undefined) {
        const chunk = this.child.stdout.read(4 - this.prefixUsed);
        if (chunk === null) return;
        chunk.copy(this.prefix, this.prefixUsed);
        this.prefixUsed += chunk.length;
        copied += chunk.length;
        if (this.prefixUsed !== 4) continue;
        this.length = this.prefix.readUInt32LE();
        this.prefixUsed = 0;
        if (!this.length || this.length > LIMITS.reply) throw new Error("Invalid query reply frame length");
      }
      if (copied >= LIMITS.readTurn) break;
      if (!this.payload) {
        if (this.pending.size >= LIMITS.replySlots || this.reservedBytes + this.length > LIMITS.replyBytes) return;
        // Admission precedes allocation. An exact ArrayBuffer prevents exposing
        // unrelated bytes from Node's pooled Buffer backing storage.
        this.reservedBytes += this.length;
        this.payload = new Uint8Array(this.length);
        this.offset = 0;
      }
      const chunk = this.child.stdout.read(Math.min(LIMITS.readChunk, this.length - this.offset, LIMITS.readTurn - copied));
      if (chunk === null) return;
      this.payload.set(chunk, this.offset);
      this.offset += chunk.length;
      copied += chunk.length;
      if (this.offset === this.length) {
        const token = (++this.replySequence).toString();
        const record = { bytes: this.length, posted: false, consumed: false };
        this.pending.set(token, record);
        const bytes = this.payload.buffer;
        this.payload = undefined;
        this.length = undefined;
        this.offset = 0;
        this.post({ type: "rpcReply", token, bytes }, record, token);
      }
    }
    this.schedule();
  }
  setVisible(visible) { this.visible = !!visible; this.schedule(); }

  fail(message) {
    if (this.disposed) return;
    this.failure = String(message).slice(0, 2048);
    try { this.onError(this.failure); } catch {}
    this.post({ type: "rpcFailed", message: this.failure });
    this.dispose();
  }
  closed(code, signal) {
    this.childClosed = true;
    this.accepting = false;
    for (const timer of this.timers) clearTimeout(timer);
    this.timers = [];
    if (!this.disposed) this.exitPending = { type: "rpcStopped", code, signal, diagnostic: this.diagnostic.toString("utf8") };
    if (!this.disposed && (this.child.stdout.readableLength || this.prefixUsed || this.length !== undefined)) {
      this.fail("Query child closed with unread or incomplete output");
    }
    this.discard();
    this.finish();
  }
  discard() {
    if (this.payload) this.reservedBytes -= this.payload.byteLength;
    this.payload = undefined;
    this.length = undefined;
    this.prefixUsed = 0;
    for (const [token, record] of this.pending) { record.consumed = true; this.release(token, record); }
  }
  dispose() {
    if (!this.disposed) {
      this.disposed = true;
      this.accepting = false;
      this.exitPending = undefined;
      this.discard();
      this.child.stdout.destroy();
      this.child.stderr.destroy();
      this.child.stdin.end();
      if (!this.childClosed) {
        for (const [delay, signal] of [[this.graceMs, "SIGTERM"], [this.killMs, "SIGKILL"]]) {
          const timer = setTimeout(() => { if (!this.childClosed) { try { this.child.kill(signal); } catch {} } }, delay);
          timer.unref?.();
          this.timers.push(timer);
        }
      }
    }
    this.finish();
    return this.stopped;
  }
  finish() {
    if (!this.childClosed || this.posts || this.sending) return;
    if (this.exitPending) {
      const message = this.exitPending;
      this.exitPending = undefined;
      this.post(message);
    } else this.resolveStopped({ failure: this.failure });
  }
  stats() {
    return { reservedBytes: this.reservedBytes, deliveries: this.pending.size,
      posts: this.posts, sendBytes: this.sending?.bytes || 0,
      streamBytes: this.child.stdout.readableLength, diagnosticBytes: this.diagnostic.length,
      childClosed: this.childClosed, disposed: this.disposed };
  }
}
module.exports = { Relay, LIMITS, serverBinary };
