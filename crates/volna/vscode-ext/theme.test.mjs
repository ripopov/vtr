import { test } from 'node:test';
import assert from 'node:assert/strict';
import { snapshot, watchTheme } from './theme.mjs';

function host(classes = [], values = {}) {
  classes.contains = c => classes.includes(c);
  const document = { body: { classList: classes, dataset: {} }, documentElement: {} };
  const style = () => Object.assign(Object.keys(values), { getPropertyValue: key => values[key] ?? '' });
  return { document, style, classes, values };
}
function observe(h) {
  const calls = [], watched = [];
  let notify, disconnected = false, pagehide;
  h.document.defaultView = { addEventListener: (name, cb) => { if (name === 'pagehide') pagehide = cb; } };
  class Observer {
    constructor(callback) { notify = callback; }
    observe(element, options) { watched.push([element, options]); }
    disconnect() { disconnected = true; }
  }
  const themes = watchTheme(s => calls.push(s), h.document, Observer, h.style);
  return { calls, watched, ...themes, notify: () => notify(), hide: () => { pagehide(); assert.ok(disconnected); } };
}

test('snapshots forward raw CSS; authoritative kind wins over legacy classes', () => {
  const h = host(['vscode-high-contrast', 'vscode-high-contrast-light'], {
    '--vscode-editor-background': '#abc', color: 'red', '--vscode-new-token': 'rgb(10% 20% 30% / 50%)',
  });
  assert.equal(snapshot(h.document, h.style), 'kind=vscode-high-contrast-light\n--vscode-editor-background=#abc\n--vscode-new-token=rgb(10% 20% 30% / 50%)');
  h.document.body.dataset.vscodeThemeKind = 'vscode-dark';
  assert.ok(snapshot(h.document, h.style).startsWith('kind=vscode-dark\n'));
});

test('initial snapshot is synchronous in all kinds, including absent metadata', () => {
  for (const kind of ['', 'vscode-dark', 'vscode-light', 'vscode-high-contrast', 'vscode-high-contrast-light']) {
    const o = observe(host(kind ? [kind] : []));
    assert.equal(o.current(), `kind=${kind}`); // WASM calls this before its first window.
    assert.equal(o.calls.length, 0); // No WASM calls before initialization.
    o.start();
    assert.equal(o.calls.length, 0); // Initial palette is not sent twice.
    o.hide();
  }
});

test('changes during startup, late metadata, same-kind changes and removal are delivered', () => {
  const h = host(), o = observe(h);
  assert.equal(o.watched.length, 2);
  assert.equal(o.current(), 'kind=');
  h.values['--vscode-editor-background'] = '#fff';
  o.notify();
  assert.equal(o.calls.length, 0);
  o.start();
  assert.equal(o.calls.at(-1), 'kind=\n--vscode-editor-background=#fff');
  h.classes.push('vscode-light');
  o.notify();
  assert.ok(o.calls.at(-1).startsWith('kind=vscode-light\n'));
  h.values['--vscode-editor-background'] = '#fedcba';
  o.notify();
  assert.ok(o.calls.at(-1).endsWith('=#fedcba'));
  delete h.values['--vscode-editor-background'];
  o.notify();
  assert.equal(o.calls.at(-1), 'kind=vscode-light');
  const count = o.calls.length;
  o.notify();
  assert.equal(o.calls.length, count);
  o.hide();
  h.classes.length = 0;
  o.notify();
  assert.equal(o.calls.length, count);
});
