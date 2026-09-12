// Minimal Chrome DevTools Protocol driver (Node >= 22, built-in WebSocket).
// Usage: node tools/cdp.mjs <port> <script.json>
//   script: [{"navigate": url} | {"wait": ms} | {"shot": "file.png"} |
//            {"click": [x, y]} | {"mousedown"/"mouseup"/"move": [x, y]} |
//            {"key": "=", "code": "Equal"} | {"wheel": [x, y, dx, dy, "ctrl"?]} |
//            {"eval": "js"} | {"target": "substring of title/url"}]
import fs from "node:fs";

const [port, scriptPath] = process.argv.slice(2);
const steps = JSON.parse(fs.readFileSync(scriptPath, "utf8"));
const list = async () => (await fetch(`http://127.0.0.1:${port}/json/list`)).json();

let targets = await list();
let want = steps.find((s) => s.target)?.target;
let page = targets.find((t) => t.type === "page" && (!want || (t.title + t.url).includes(want))) ?? targets.find((t) => t.type === "page");
if (!page) { console.error("no page target", targets); process.exit(1); }
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
const logs = [];
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  else if (m.method === "Runtime.consoleAPICalled") logs.push(`[console.${m.params.type}] ${m.params.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
  else if (m.method === "Runtime.exceptionThrown") logs.push(`[exception] ${m.params.exceptionDetails.text} ${m.params.exceptionDetails.exception?.description ?? ""}`);
  else if (m.method === "Log.entryAdded") logs.push(`[log.${m.params.entry.level}] ${m.params.entry.text}`);
};
const send = (method, params = {}) => new Promise((res) => { const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
await send("Page.enable"); await send("Runtime.enable"); await send("Log.enable");
await send("Network.enable"); await send("Network.setCacheDisabled", { cacheDisabled: true });

for (const s of steps) {
  if (s.navigate) { await send("Page.navigate", { url: s.navigate }); }
  else if (s.wait) await sleep(s.wait);
  else if (s.shot) {
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(s.shot, Buffer.from(r.result.data, "base64"));
    console.log("saved", s.shot);
  } else if (s.click || s.mousedown || s.mouseup || s.move) {
    const [x, y] = s.click ?? s.mousedown ?? s.mouseup ?? s.move;
    const mods = s.shift ? 8 : 0;
    const ev = (type, extra = {}) => send("Input.dispatchMouseEvent", { type, x, y, button: s.button ?? "left", modifiers: mods, ...extra });
    if (s.move || s.click) await ev("mouseMoved", { button: "none" });
    if (s.click || s.mousedown) await ev("mousePressed", { clickCount: s.count ?? 1 });
    if (s.click || s.mouseup) await ev("mouseReleased", { clickCount: s.count ?? 1 });
    if (s.click && (s.count ?? 1) === 2) { await ev("mousePressed", { clickCount: 2 }); await ev("mouseReleased", { clickCount: 2 }); }
    await sleep(60);
  } else if (s.key !== undefined) {
    const mods = (s.shift ? 8 : 0) | (s.ctrl ? 2 : 0) | (s.meta ? 4 : 0) | (s.alt ? 1 : 0);
    const base = { key: s.key, code: s.code ?? "", modifiers: mods, windowsVirtualKeyCode: s.vk ?? 0, nativeVirtualKeyCode: s.vk ?? 0 };
    await send("Input.dispatchKeyEvent", { type: s.key.length === 1 ? "keyDown" : "rawKeyDown", text: s.key.length === 1 && !mods ? s.key : undefined, ...base });
    await send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
    await sleep(60);
  } else if (s.wheel) {
    const [x, y, dx, dy, mod] = s.wheel;
    await send("Input.dispatchMouseEvent", { type: "mouseWheel", x, y, deltaX: dx, deltaY: dy, modifiers: mod === "ctrl" ? 2 : mod === "shift" ? 8 : 0 });
    await sleep(60);
  } else if (s.eval) {
    const r = await send("Runtime.evaluate", { expression: s.eval, awaitPromise: true, returnByValue: true });
    console.log("eval:", JSON.stringify(r.result.result?.value ?? r.result));
  }
}
for (const l of logs) console.log(l);
ws.close();
