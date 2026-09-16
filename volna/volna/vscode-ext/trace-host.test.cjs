const test = require("node:test");
const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const { createTraceHost, FrameReader, packetBytes } = require("./trace-host.cjs");

function frame(size = 10) {
  const bytes = Buffer.alloc(12 + size, 7);
  bytes.write("VLNA"); bytes.writeUInt32LE(1, 4); bytes.writeUInt32LE(size, 8);
  return bytes;
}
function fixture(postResult = true) {
  const messages = [], children = [], launches = [];
  const vscode = { workspace: { getConfiguration: () => ({ get: () => "" }) } };
  const panel = { webview: { postMessage: async message => { messages.push(message); return postResult; } } };
  const context = { extensionPath: "/tmp/test extension" };
  const uri = { scheme: "vscode-remote", fsPath: "/remote/a file.vtr" };
  const spawn = (...args) => {
    launches.push(args);
    const child = new EventEmitter(); child.stdout = new EventEmitter(); child.stderr = new EventEmitter(); child.stdin = new EventEmitter();
    child.written = []; child.killed = false;
    child.stdin.destroy = () => {};
    child.stdin.write = (bytes, cb) => { child.written.push(Buffer.from(bytes)); cb(); return true; };
    child.kill = () => { child.killed = true; };
    children.push(child); return child;
  };
  return { host: createTraceHost(vscode,context,panel,uri,spawn), messages,children,launches };
}
const tick = () => new Promise(resolve => setImmediate(resolve));

test("frame reader handles arbitrary pipe fragmentation without extra bytes", () => {
  const original = frame(4096);
  for (const chunk of [1,7,12,53,10000]) {
    const received = []; const reader = new FrameReader(bytes => received.push(bytes));
    for (let i=0; i<original.length; i+=chunk) reader.push(original.subarray(i,i+chunk));
    reader.end(); assert.deepEqual(received,[original]);
  }
  for (const cut of [1,11,12,4096]) {
    const reader = new FrameReader(() => assert.fail("truncated frame delivered"));
    reader.push(original.subarray(0,cut)); assert.throws(() => reader.end(),/inside a frame/);
  }
  const oversized = frame(); oversized.writeUInt32LE(0xffffffff,8);
  assert.throws(() => new FrameReader(()=>{}).push(oversized),/size limit/);
});

test("relay sends exact ArrayBuffers and forwards acknowledgements", async () => {
  const f = fixture(); await f.host.receive({type:"traceStart",connection:"A"});
  assert.equal(f.launches[0][0],"/tmp/test extension/bin/volna-server");
  assert.deepEqual(f.launches[0][1],["/remote/a file.vtr"]);
  assert.equal(f.launches[0][2].shell,false);
  const [child] = f.children; const bytes = frame();
  child.stdout.emit("data",bytes); await tick();
  assert.equal(f.messages[0].type,"traceFrame");
  assert.ok(f.messages[0].bytes instanceof ArrayBuffer);
  assert.deepEqual(Buffer.from(f.messages[0].bytes),bytes);
  await f.host.receive({type:"traceRequest",connection:"A",bytes:Array.from(bytes)});
  assert.deepEqual(child.written,[bytes]);
  child.stdout.emit("data",bytes); await tick();
  assert.equal(f.messages.length,2);
  f.host.dispose(); assert.ok(child.killed);
});

test("old child callbacks and requests cannot affect a replacement", async () => {
  const f=fixture(); await f.host.receive({type:"traceStart",connection:"A"});
  const old=f.children[0]; await f.host.receive({type:"traceStart",connection:"B"});
  assert.ok(old.killed);
  old.emit("exit",1); old.stdout.emit("data",frame()); old.emit("error",new Error("old"));
  await f.host.receive({type:"traceRequest",connection:"A",bytes:frame()});
  assert.equal(f.messages.length,0); assert.equal(f.children[1].written.length,0);
  f.host.dispose(); assert.ok(f.children[1].killed);
});

test("failed delivery and unsolicited second frames terminate the child", async () => {
  const f=fixture(false); await f.host.receive({type:"traceStart",connection:"A"});
  f.children[0].stdout.emit("data",frame()); await tick();
  assert.ok(f.children[0].killed); assert.equal(f.messages[1].type,"traceError");
  const g=fixture(); await g.host.receive({type:"traceStart",connection:"A"});
  g.children[0].stdout.emit("data",Buffer.concat([frame(),frame()])); await tick();
  assert.ok(g.children[0].killed); assert.match(g.messages[1].message,/acknowledgement/);
});

test("partial child output and invalid client packets report failures", async () => {
  const f=fixture(); await f.host.receive({type:"traceStart",connection:"A"});
  f.children[0].stdout.emit("data",frame().subarray(0,13)); f.children[0].emit("exit",1); await tick();
  assert.match(f.messages[0].message,/inside a frame/);
  assert.throws(()=>packetBytes([1,2,-1]),/invalid client/);
  assert.throws(()=>packetBytes(frame().subarray(0,14)),/length/);
  const g=fixture(); await g.host.receive({type:"traceStart",connection:"A"});
  await g.host.receive({type:"traceRequest",connection:"A",bytes:[0]}); await tick();
  assert.ok(g.children[0].killed); assert.equal(g.messages[0].type,"traceError");
});
