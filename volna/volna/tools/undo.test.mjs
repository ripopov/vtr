// node --test volna/volna/tools/undo.test.mjs   (after volna/volna/web/build.sh)
// Owns a headless browser and loopback server for the real web bundle; the
// assertions are the gate. Checks that the platform undo keys reach the
// cockpit history in the wasm page, that the page keeps the browser's own
// Edit ▸ Undo off it (the key's default is prevented), and that the host's
// named commands take the same path (volna/volna/ARCHITECTURE.md, "Undo and redo").
import assert from 'node:assert/strict';
import {readdir, readFile, stat} from 'node:fs/promises';
import {join, relative} from 'node:path';
import {test} from 'node:test';
import {fileURLToPath} from 'node:url';
import {browserTest, until} from './browser-test.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const dist = join(root, 'web', 'dist');
const examples = join(root, 'examples');
const TYPES = {'.js': 'text/javascript', '.mjs': 'text/javascript', '.wasm': 'application/wasm',
  '.html': 'text/html', '.json': 'application/json', '.vtr': 'application/octet-stream'};

async function files(dir) {
  const out = [];
  for (const entry of await readdir(dir, {withFileTypes: true})) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...await files(path));
    else out.push(path);
  }
  return out;
}

async function routes() {
  await stat(join(dist, 'volna_bg.wasm')).catch(() => {
    throw new Error('web/dist is missing: run volna/volna/web/build.sh first');
  });
  const out = {'/': {type: 'text/html', body: await readFile(join(root, 'web', 'index.html'))}};
  for (const path of await files(dist)) {
    const type = TYPES[path.slice(path.lastIndexOf('.'))] ?? 'application/octet-stream';
    out[`/dist/${relative(dist, path).split('\\').join('/')}`] = {type, body: await readFile(path)};
  }
  for (const name of ['landing.vtr', 'landing.vtr.volna.json']) {
    out[`/examples/${name}`] = {type: TYPES[name.slice(name.lastIndexOf('.'))], body: await readFile(join(examples, name))};
  }
  return out;
}

/** The viewer's one-line state per panel, as `debug_state()` logs it. */
async function state(b) {
  return b.evaluate(`new Promise((resolve, reject) => {
    const original = console.info;
    const timer = setTimeout(() => { console.info = original; reject(new Error('no viewer state')); }, 3000);
    console.info = (...args) => {
      original(...args);
      const text = args.join(' ');
      if (text.includes('STATE ')) { clearTimeout(timer); console.info = original; resolve(text); }
    };
    window.volnaModule.debug_state();
  })`);
}

/** Rows of wave panel 1 and whether it has keyboard focus in the core. */
async function waves(b) {
  const line = (await state(b)).split('\n').find(l => l.includes('panel=1 focused='));
  assert.ok(line, 'wave panel 1 in the viewer state');
  return {items: +line.match(/ items=(\d+)/)[1], focused: line.includes('focused=true')};
}

const MODIFIERS = {alt: 1, ctrl: 2, meta: 4, shift: 8};

/** Press a key chord like `ctrl+shift+z` as the keyboard would. */
async function press(b, chord) {
  const parts = chord.split('+');
  const key = parts.pop();
  const modifiers = parts.reduce((m, p) => m | MODIFIERS[p], 0);
  const named = {Delete: {code: 'Delete', keyCode: 46}};
  const info = named[key] ?? {code: `Key${key.toUpperCase()}`, keyCode: key.toUpperCase().charCodeAt(0)};
  const text = key.length === 1 && !(modifiers & (MODIFIERS.ctrl | MODIFIERS.meta)) ? key : undefined;
  const event = {modifiers, key: (modifiers & MODIFIERS.shift) && key.length === 1 ? key.toUpperCase() : key,
    code: info.code, windowsVirtualKeyCode: info.keyCode, nativeVirtualKeyCode: info.keyCode};
  await b.send('Input.dispatchKeyEvent', {type: 'keyDown', text, ...event});
  await b.send('Input.dispatchKeyEvent', {type: 'keyUp', ...event});
}

test('undo keys reach the cockpit history and never the browser', {timeout: 180000}, async t => {
  const b = await browserTest(await routes(), {
    graphics: true,
    ready: `!!document.querySelector('canvas')`,
    readyTimeout: 60000,
  });
  t.after(() => b.close());
  await b.evaluate(`import('./dist/volna.js').then(m => { window.volnaModule = m; return true; })`);
  // The page loads the landing trace with its workspace, as ?file= would,
  // once the viewer takes host calls.
  await b.evaluate(`(async () => {
    const trace = new Uint8Array(await (await fetch('/examples/landing.vtr')).arrayBuffer());
    const saved = Array.from(new Uint8Array(await (await fetch('/examples/landing.vtr.volna.json')).arrayBuffer()));
    const traceUri = new URL('/examples/landing.vtr', location.href).href;
    window.openLanding = () => {
      try {
        window.volnaModule.open_resource('landing.vtr', trace, JSON.stringify({
          traceUri,
          candidates: {
            sidecar: {target: {kind: 'file', uri: traceUri + '.volna.json'}, content: {status: 'bytes', value: saved}, writable: false},
            fallback: {target: {kind: 'storage', key: 'volna.workspace:' + traceUri}, content: {status: 'missing'}, writable: false},
          },
        }));
        return true;
      } catch (error) {
        if (String(error).includes('not ready')) return false;
        throw error;
      }
    };
    return true;
  })()`);
  await until(() => b.evaluate('window.openLanding()'), 'the viewer takes the trace', 60000);
  const loaded = await until(async () => {
    const w = await waves(b).catch(() => null);
    return w && w.items > 0 ? w : null;
  }, 'the landing workspace restores its wave rows', 60000);

  // Record whether each key's default was prevented once the page handled
  // it, and what the page announces to assistive technology.
  await b.evaluate(`window.prevented = [];
    window.addEventListener('keydown', e => setTimeout(() => window.prevented.push([e.key, e.defaultPrevented])));
    window.announced = [];
    document.ariaNotify = text => window.announced.push(text);
    true`);
  // Focus wave panel 1 and click a row: keyboard focus follows the click.
  await b.evaluate(`window.volnaModule.dispatch_command('focusPanel1'); true`);
  await until(async () => (await waves(b)).focused, 'wave panel 1 focused');
  await b.send('Input.dispatchMouseEvent', {type: 'mousePressed', x: 700, y: 300, button: 'left', clickCount: 1});
  await b.send('Input.dispatchMouseEvent', {type: 'mouseReleased', x: 700, y: 300, button: 'left', clickCount: 1});

  await press(b, 'ctrl+a');
  await press(b, 'Delete');
  await until(async () => (await waves(b)).items === 0, 'Delete removes the selected rows');

  await b.evaluate('window.prevented = []; true');
  await press(b, 'ctrl+z');
  await until(async () => (await waves(b)).items === loaded.items, 'Ctrl+Z puts the rows back');
  const undone = await b.evaluate('window.prevented');
  assert.deepEqual(undone.filter(([key]) => key === 'z'), [['z', true]], 'Ctrl+Z is not the browser\'s');
  assert.match((await b.evaluate('window.announced')).at(-1), /^Undid Remove \d+ rows$/, 'undo is announced');

  await press(b, 'ctrl+y');
  await until(async () => (await waves(b)).items === 0, 'Ctrl+Y redoes');
  await press(b, 'ctrl+z');
  await until(async () => (await waves(b)).items === loaded.items, 'undone again');
  await press(b, 'ctrl+shift+z');
  await until(async () => (await waves(b)).items === 0, 'Ctrl+Shift+Z redoes');

  // A host's named commands (VS Code forwards its keys this way).
  await b.evaluate(`window.volnaModule.dispatch_command('undo'); true`);
  await until(async () => (await waves(b)).items === loaded.items, 'the host command undoes');
  await b.evaluate(`window.volnaModule.dispatch_command('redo'); true`);
  await until(async () => (await waves(b)).items === 0, 'the host command redoes');
  assert.deepEqual(b.exceptions, []);
});
