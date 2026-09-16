// Capture renderer/GPU timing while replaying a cdp.mjs script (Node >= 22).
// Usage: node tools/trace-cdp.mjs PORT SCRIPT.json OUTPUT.json
// The replay console is retained beside the trace as OUTPUT.json.log.
import fs from "node:fs";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const [port, script, output] = process.argv.slice(2);
if (!port || !script || !output) throw new Error("Expected PORT SCRIPT.json OUTPUT.json");
const version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
const ws = new WebSocket(version.webSocketDebuggerUrl);
await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
let id = 0, finished;
const pending = new Map();
const completion = new Promise(resolve => { finished = resolve; });
ws.onmessage = event => {
  const message = JSON.parse(event.data);
  if (pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  } else if (message.method === "Tracing.tracingComplete") {
    finished(message.params.stream);
  }
};
const send = (method, params = {}) => new Promise((resolve, reject) => {
  const key = ++id;
  pending.set(key, message => message.error
    ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result));
  ws.send(JSON.stringify({ id: key, method, params }));
});
try {
  await send("Tracing.start", {
    categories: "devtools.timeline,blink,cc,viz,gpu", transferMode: "ReturnAsStream",
  });
  const log = fs.openSync(output + ".log", "w");
  try {
    await new Promise((resolve, reject) => {
      const child = spawn(process.execPath,
        [fileURLToPath(new URL("./cdp.mjs", import.meta.url)), port, script],
        { stdio: ["ignore", log, log] });
      child.on("error", reject);
      child.on("exit", code => code === 0 ? resolve() : reject(new Error(`Replay exited ${code}`)));
    });
  } finally {
    fs.closeSync(log);
    await send("Tracing.end");
    const handle = await completion;
    const file = fs.openSync(output, "w");
    try {
      for (;;) {
        const chunk = await send("IO.read", { handle, size: 1048576 });
        fs.writeSync(file, chunk.base64Encoded ? Buffer.from(chunk.data, "base64") : chunk.data);
        if (chunk.eof) break;
      }
    } finally {
      fs.closeSync(file);
      await send("IO.close", { handle });
    }
  }
} finally {
  ws.close();
}
