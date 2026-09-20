import assert from "node:assert/strict";
import test from "node:test";
import { copyTableText } from "../src/table_clipboard.mjs";

class Element {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.dataset = {};
    this.style = {};
    this.listeners = new Map();
    this.isConnected = false;
  }
  setAttribute(name, value) { this[name] = value; }
  addEventListener(name, fn) { this.listeners.set(name, fn); }
  append(...children) { this.children.push(...children); }
  focus() { this.focused = true; }
  select() { this.selected = true; }
  showModal() { this.open = true; }
  close() { this.open = false; }
  remove() { this.isConnected = false; this.removed = true; }
  click() { this.listeners.get("click")?.({}); }
}

const turn = () => new Promise(resolve => setTimeout(resolve, 0));

test("clipboard rejection preserves complete TSV and Retry succeeds", async () => {
  const body = new Element("body");
  body.append = (...children) => {
    for (const child of children) child.isConnected = true;
    body.children.push(...children);
  };
  const document = {
    body,
    activeElement: null,
    createElement: tag => new Element(tag),
    querySelector: () => null,
  };
  let attempts = 0;
  const host = {
    document,
    navigator: { clipboard: { writeText: async () => {
      attempts += 1;
      if (attempts === 1) throw new Error("denied");
    } } },
  };
  const text = "Time\ta\n18446744073709551615\t1";
  copyTableText(text, host);
  await turn();

  const dialog = body.children[0];
  const area = dialog.children.find(child => child.tagName === "TEXTAREA");
  const retry = dialog.children.find(child => child.textContent === "Retry copy");
  assert.equal(dialog.open, true);
  assert.equal(area.value, text);
  assert.equal(area.selected, true);

  retry.click();
  await turn();
  assert.equal(attempts, 2);
  assert.equal(dialog.removed, true);
  assert.equal(area.value, "");
});
