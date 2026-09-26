// node --test volna/volna/tools/frame-stats.test.mjs   (after volna/volna/web/build.sh)
// Owns a headless browser and loopback server for the real web bundle; the
// assertions are the gate. Checks GPUI's frame profiler reaches the status
// bar on the web platform, and that an untouched window stops redrawing.
import assert from 'node:assert/strict';
import {readdir, readFile, stat} from 'node:fs/promises';
import {join, relative} from 'node:path';
import {test} from 'node:test';
import {setTimeout as delay} from 'node:timers/promises';
import {fileURLToPath} from 'node:url';
import {browserTest, until} from './browser-test.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const dist = join(root, 'web', 'dist');
const TYPES = {'.js': 'text/javascript', '.mjs': 'text/javascript', '.wasm': 'application/wasm', '.html': 'text/html'};

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
  return out;
}

// The `frames …` line that `debug_state()` logs after the panel lines.
async function frames(b) {
  const text = await b.evaluate(`new Promise((resolve, reject) => {
    const original = console.info;
    const timer = setTimeout(() => { console.info = original; reject(new Error('no viewer state')); }, 3000);
    console.info = (...args) => {
      original(...args);
      const text = args.join(' ');
      if (text.includes('STATE ')) { clearTimeout(timer); console.info = original; resolve(text); }
    };
    window.volnaModule.debug_state();
  })`);
  const line = text.split('\n').find(l => l.startsWith('frames '));
  assert.ok(line, `frames line in ${text}`);
  const m = line.match(/recent=(\d+) total=(\d+) status=(None|Some\("(.*)"\)) level=\S+ idle=(true|false)/);
  assert.ok(m, `parsable frames line: ${line}`);
  return {line, recent: +m[1], total: +m[2], status: m[4] ?? null, idle: m[5] === 'true'};
}

test('web frame timing reaches the status bar and settles when idle', {timeout: 120000}, async t => {
  const b = await browserTest(await routes(), {graphics: true, ready: `!!document.querySelector('canvas')`, readyTimeout: 60000});
  t.after(() => b.close());
  await b.evaluate(`import('./dist/volna.js').then(m => { window.volnaModule = m; return true; })`);

  // Pointer movement over the window makes hover redraws: real frames.
  for (let i = 0; i < 40; i++) {
    await b.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: 100 + i * 20, y: 120 + (i % 5) * 60, button: 'none'});
    await delay(25);
  }
  const active = await until(async () => {
    const f = await frames(b);
    return f.recent > 0 && f.status ? f : null;
  }, 'frames sampled into the status', 10000);
  assert.match(active.status, /^draw \d+(\.\d)? · lat \d+(\.\d)? ms$/, active.line);

  // Untouched, the status dims, and its own redraws do not keep it busy.
  const idle = await until(async () => {
    const f = await frames(b);
    return f.idle ? f : null;
  }, 'idle status', 10000);
  await delay(2000);
  const later = await frames(b);
  assert.equal(later.total, idle.total, `no frames of its own while idle: ${idle.line} → ${later.line}`);
  assert.equal(later.idle, true);
  assert.deepEqual(b.exceptions, []);
});
