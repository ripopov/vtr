// Lifecycle of a native query child for one custom-editor resource. The webview
// supplies opaque packets, never executable paths or a replacement trace URI.
const { Relay, serverBinary } = require("./relay");

function createQueryHost(context, panel, uri, { makeRelay = options => new Relay(options) } = {}) {
  let relay;
  let generation = 0;
  let disposed = false;
  let stopping = Promise.resolve();
  function retire() {
    if (relay) {
      const old = relay;
      relay = undefined;
      stopping = old.dispose();
    }
    return stopping;
  }
  async function open(metadata) {
    const current = ++generation;
    await retire();
    if (disposed || current !== generation) return;
    if (!["file", "vscode-remote"].includes(uri.scheme)) {
      throw new Error(`Native trace queries do not support ${uri.scheme} resources`);
    }
    const child = makeRelay({
      binary: serverBinary(context.extensionPath),
      tracePath: uri.fsPath,
      visible: panel.visible,
      postMessage: message => !disposed && current === generation
        ? panel.webview.postMessage(message) : Promise.resolve(false),
    });
    relay = child;
    try {
      const delivered = await panel.webview.postMessage({ ...metadata, type: "rpcOpen", incarnation: child.incarnation });
      if (!delivered && relay === child) await retire();
    } catch (error) {
      if (relay === child) await retire();
      throw error;
    }
  }
  return {
    open,
    receive(message) {
      if (message?.type === "rpcClose" && message.incarnation === relay?.incarnation) {
        ++generation;
        void retire();
        return true;
      }
      return relay?.receive(message) || false;
    },
    visibility() { relay?.setVisible(panel.visible); },
    dispose() { disposed = true; ++generation; return retire(); },
  };
}
module.exports = { createQueryHost };
