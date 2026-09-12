import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { rgba, snapshot, followTheme } from './theme.mjs';

function host(classes = [], values = {}) {
  const document = { body: { classList: { contains: c => classes.includes(c) }, dataset: {} }, documentElement: {} };
  const style = () => ({ getPropertyValue: key => values[key] ?? '' });
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
  const ready = followTheme((...args) => calls.push(args), h.document, Observer, h.style);
  return { calls, watched, ready, notify: () => notify(), dispose: () => { pagehide(); assert.ok(disconnected); } };
}

test('CSS hex/rgb/rgba and missing/malformed tokens', () => {
  for (const [css, expected] of [['#123', 0x112233ff], ['#1234', 0x11223344], [' #AbCdEf ', 0xabcdefff], ['#12345678', 0x12345678], ['rgba(100, 100, 100, 0.4)', 0x64646466], ['rgb(1, 2, 3)', 0x010203ff], ['rgba(0, 0, 0, 0)', 0]]) assert.equal(rgba(css), expected);
  for (const value of ['', 'inherit', '#nope', '#12345', '123456', 'rgba(256, 0, 0, 1)', 'rgba(1, 2, 3, 2)', 'rgb(1, 2, 3, 1)', 'rgba(1, 2, 3)', 'rgb(., 0, 0)']) assert.equal(rgba(value), undefined);
});

test('initial metadata is applied synchronously in all four modes', async () => {
  for (const [classes, appearance] of [[['vscode-light'], 'Light'], [['vscode-dark'], 'Dark'], [['vscode-high-contrast'], 'HighContrastDark'], [['vscode-high-contrast', 'vscode-high-contrast-light'], 'HighContrastLight']]) {
    const h = host(classes, { '--vscode-editor-background': '#123456', '--vscode-charts-green': '#abcd' });
    const o = observe(h);
    assert.equal(o.calls.length, 1); // Before any await or startup call.
    assert.equal(o.calls[0][0], appearance);
    assert.equal(o.calls[0][1].editor.background, 0x123456ff);
    assert.equal(o.calls[0][1].charts.green, 0xaabbccdd);
    await o.ready;
    o.dispose();
  }
});

test('delayed metadata, same-kind changes, removal and deduplication', async () => {
  const h = host(), o = observe(h);
  let ready = false;
  o.ready.then(() => { ready = true; });
  await Promise.resolve();
  assert.equal(ready, false);
  assert.equal(o.watched.length, 2);
  h.classes.push('vscode-light');
  h.values['--vscode-editor-background'] = '#fff';
  o.notify();
  await o.ready;
  assert.equal(o.calls.length, 1);
  o.notify();
  assert.equal(o.calls.length, 1);
  h.values['--vscode-editor-background'] = '#fedcba';
  o.notify();
  assert.equal(o.calls.at(-1)[1].editor.background, 0xfedcbaff);
  delete h.values['--vscode-editor-background'];
  o.notify();
  assert.equal(o.calls.at(-1)[1].editor.background, undefined);
  h.document.body.dataset.vscodeThemeKind = 'vscode-high-contrast-light';
  o.notify();
  assert.equal(o.calls.at(-1)[0], 'HighContrastLight');
  o.dispose();
});

test('missing metadata has bounded startup and late metadata still wins', async () => {
  for (const [bg, expected] of [['#fff4e6', 'Light'], ['#123456', 'Dark'], ['', 'Dark']]) {
    const h = host([], { '--vscode-editor-background': bg }), o = observe(h);
    await o.ready;
    assert.equal(o.calls.at(-1)[0], expected);
    assert.equal(o.calls.at(-1)[1].editor.background, rgba(bg));
    h.classes.push('vscode-high-contrast-light');
    o.notify();
    assert.equal(o.calls.at(-1)[0], 'HighContrastLight');
    o.dispose();
  }
});

// Generate typed Rust inputs from exactly the same real snapshots/mapping tested
// here. Checked-in output lets native tests consume them without Node or a parser.
const fixtures = ['dark-modern', 'light-modern', 'dark-high-contrast', 'light-high-contrast'];
const dir = new URL('../tests/fixtures/vscode/', import.meta.url);
const rustColor = value => value === undefined ? 'None' : `Some(gpui::rgba(0x${value.toString(16).padStart(8, '0')}).into())`;
function rustPalette({ appearance, colors }) {
  const fields = Object.entries(colors).map(([name, color]) => {
    const value = name === 'charts' ? `[${Object.values(color).map(rustColor).join(', ')}]`
      : typeof color === 'object' ? `ColorPair { background: ${rustColor(color.background)}, foreground: ${rustColor(color.foreground)} }` : rustColor(color);
    return `            ${name}: ${value},`;
  });
  return `        HostPalette {\n            appearance: Appearance::${appearance},\n${fields.join('\n')}\n        }`;
}
test('real VS Code snapshots map every token and keep typed Rust fixtures current', async () => {
  const palettes = [];
  for (const name of fixtures) {
    const raw = await readFile(new URL(`${name}.txt`, dir), 'utf8');
    const entries = Object.fromEntries(raw.split('\n').filter(l => l.includes('=')).map(l => [l.slice(0, l.indexOf('=')), l.slice(l.indexOf('=') + 1)]));
    const h = host([entries.kind], entries);
    const p = snapshot(h.document, h.style);
    assert.ok(p);
    for (const [pair, token] of [['editor', 'editor'], ['panel', 'sideBar'], ['selection', 'list-activeSelection'], ['button', 'button']]) {
      const sep = pair === 'selection' ? '' : '-';
      assert.equal(p.colors[pair].background, rgba(entries[`--vscode-${token}${sep}${pair === 'selection' ? 'Background' : 'background'}`] ?? ''));
      assert.equal(p.colors[pair].foreground, rgba(entries[`--vscode-${token}${sep}${pair === 'selection' ? 'Foreground' : 'foreground'}`] ?? ''));
    }
    assert.ok(p.colors.charts.green !== undefined);
    assert.ok(p.colors.scrollbar !== undefined);
    palettes.push(rustPalette(p));
  }
  const output = `// Generated by UPDATE_THEME_FIXTURES=1 node --test vscode-ext/theme.test.mjs.\n// Input: the four VS Code 1.137.0 snapshots in this directory.\nfn real_palettes() -> [HostPalette; 4] {\n    [\n${palettes.join(',\n')}\n    ]\n}\n`;
  const path = new URL('palettes.rs', dir);
  if (process.env.UPDATE_THEME_FIXTURES === '1') await writeFile(path, output);
  assert.equal(await readFile(path, 'utf8'), output, 'regenerate the semantic fixtures after changing the host mapping');
});
