const test = require("node:test");
const assert = require("node:assert/strict");
const { createQueryHost } = require("./query-host");

function fixture() {
  const children = [], posted = [];
  const panel = { visible: true, webview: { postMessage: async message => { posted.push(message); return true; } } };
  const host = createQueryHost({ extensionPath: "/extension" }, panel,
    { scheme: "vscode-remote", fsPath: "/remote/trace.vtr" }, {
      makeRelay(options) {
        let stopped;
        const child = { options, incarnation: String(children.length + 1).padStart(32, "0"), received: [],
          stopped: new Promise(resolve => stopped = resolve),
          receive(message) { if (message.incarnation !== this.incarnation) return false; this.received.push(message); return true; },
          dispose() { this.disposed = true; return this.stopped; },
          setVisible(value) { this.visible = value; },
          finish() { stopped(); },
        };
        children.push(child);
        return child;
      },
    });
  return { host, panel, children, posted };
}

test("resource identity stays host-owned and opaque messages use the current incarnation", async () => {
  const f = fixture();
  await f.host.open({ traceUri: "vscode-remote://ssh/remote/trace.vtr", name: "trace.vtr", settings: {} });
  const child = f.children[0];
  assert.equal(child.options.tracePath, "/remote/trace.vtr");
  assert.match(child.options.binary, /\/extension\/bin\/.*\/vtr-server(?:\.exe)?$/);
  assert.equal(f.posted[0].type, "rpcOpen");
  assert.equal(f.posted[0].incarnation, child.incarnation);
  assert.equal("bytes" in f.posted[0], false);
  const packet = { type: "rpcSend", incarnation: child.incarnation, bytes: new ArrayBuffer(12), tracePath: "/untrusted" };
  assert.equal(f.host.receive(packet), true);
  assert.equal(child.received[0], packet);
  assert.equal(child.options.tracePath, "/remote/trace.vtr");
  f.panel.visible = false;
  f.host.visibility();
  assert.equal(child.visible, false);
  const stop = f.host.dispose();
  child.finish();
  await stop;
  assert.equal(f.host.receive(packet), false);
});

test("reload waits for the old child and superseded opens cannot create extra children", async () => {
  const f = fixture();
  await f.host.open({ name: "initial" });
  const old = f.children[0];
  const first = f.host.open({ name: "superseded" });
  const second = f.host.open({ name: "current" });
  await Promise.resolve();
  assert.equal(f.children.length, 1);
  assert.equal(old.disposed, true);
  old.finish();
  await Promise.all([first, second]);
  assert.equal(f.children.length, 2);
  assert.deepEqual(f.posted.map(message => message.name), ["initial", "current"]);
  assert.equal(f.host.receive({ type: "rpcClose", incarnation: old.incarnation }), false);
  const stop = f.host.dispose();
  f.children[1].finish();
  await stop;
});

test("disposal during reload prevents a replacement child", async () => {
  const f = fixture();
  await f.host.open({});
  const reload = f.host.open({});
  const disposed = f.host.dispose();
  f.children[0].finish();
  await Promise.all([reload, disposed]);
  assert.equal(f.children.length, 1);
});
