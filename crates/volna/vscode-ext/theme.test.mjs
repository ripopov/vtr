import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rgba, snapshot, followTheme } from './theme.mjs';

function host(classes = [], values = {}) {
  const document = { body: { classList: { contains: c => classes.includes(c) } }, documentElement: {} };
  const style = () => ({ getPropertyValue: key => values[key] ?? '' });
  return { document, style, classes, values };
}

test('CSS RGBA and missing/malformed tokens', () => {
  for (const [css, expected] of [['#123', 0x112233ff], ['#1234', 0x11223344], [' #AbCdEf ', 0xabcdefff], ['#12345678', 0x12345678], ['rgba(100, 100, 100, 0.4)', 0x64646466], ['rgb(1, 2, 3)', 0x010203ff], ['rgba(0, 0, 0, 0)', 0]]) assert.equal(rgba(css), expected);
  for (const value of ['', 'inherit', '#nope', '#12345', '123456', 'rgba(256, 0, 0, 1)', 'rgba(1, 2, 3, 2)', 'rgb(1, 2, 3, 1)', 'rgba(1, 2, 3)', 'rgb(., 0, 0)']) assert.equal(rgba(value), undefined);
});

test('all four kinds, with high contrast light legacy class', () => {
  for (const [classes, dark, hc] of [[['vscode-light'], false, false], [['vscode-dark'], true, false], [['vscode-high-contrast'], true, true], [['vscode-high-contrast', 'vscode-high-contrast-light'], false, true]]) {
    const h = host(classes, { '--vscode-editor-background': '#123456', '--vscode-charts-green': '#abcd' });
    assert.deepEqual(snapshot(h.document, h.style), { dark, hc, colors: { bg_editor: 0x123456ff, wave_signal: 0xaabbccdd, success: 0xaabbccdd } });
  }
});

test('waits for startup metadata, updates within a kind, drops removed tokens, deduplicates', async () => {
  const h = host();
  let notify;
  const watched = [];
  class Observer {
    constructor(callback) { notify = callback; }
    observe(element, options) { watched.push([element, options]); }
  }
  const calls = [];
  let ready = false;
  const pending = followTheme((...args) => calls.push(args), h.document, Observer, h.style).then(() => { ready = true; });
  await Promise.resolve();
  assert.equal(ready, false);
  assert.equal(watched.length, 2);
  h.classes.push('vscode-light');
  h.values['--vscode-editor-background'] = '#fff';
  notify();
  await pending;
  assert.equal(calls.length, 1);
  notify();
  assert.equal(calls.length, 1);
  h.values['--vscode-editor-background'] = '#fedcba';
  notify();
  assert.equal(calls.at(-1)[2].bg_editor, 0xfedcbaff);
  delete h.values['--vscode-editor-background'];
  notify();
  assert.deepEqual(calls.at(-1)[2], {});
  h.classes.splice(0, 1, 'vscode-high-contrast-light', 'vscode-high-contrast');
  notify();
  assert.deepEqual(calls.at(-1).slice(0, 2), [false, true]);
});
