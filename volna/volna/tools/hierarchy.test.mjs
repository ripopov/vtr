// Build with VOLNA_WEB_FEATURES=remote-profile volna/volna/web/build.sh,
// cargo build -p volna-server --profile viewer --example hierarchy_fixture;
// node --test volna/volna/tools/hierarchy.test.mjs
// Owns temporary fixtures, a production child, loopback server and headless
// browser. The same WASM viewer opens VTR/FST locally and through protocol v5.
import assert from 'node:assert/strict';
import {spawn, execFile} from 'node:child_process';
import {mkdtemp, readdir, readFile, rm, stat} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, relative, extname} from 'node:path';
import {test} from 'node:test';
import {promisify} from 'node:util';
import {fileURLToPath} from 'node:url';
import {setTimeout as delay} from 'node:timers/promises';
import {browserTest, until} from './browser-test.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const target = process.env.CARGO_TARGET_DIR ?? join(root, '../../target');
const serverBinary = join(target, 'viewer/volna-server');
const fixtureBinary = join(target, 'viewer/examples/hierarchy_fixture');
const types = {'.js': 'text/javascript', '.mjs': 'text/javascript', '.json': 'application/json', '.wasm': 'application/wasm', '.html': 'text/html'};
async function routes(dir) {
  const out = {'/': {type: 'text/html', body: await readFile(join(root, 'web/index.html'))}};
  async function visit(path) {
    for (const entry of await readdir(path, {withFileTypes: true})) {
      const file = join(path, entry.name);
      if (entry.isDirectory()) await visit(file);
      else out[`/${relative(join(root, 'web'), file).split('\\').join('/')}`] = {type: types[extname(file)] ?? 'application/octet-stream', body: await readFile(file)};
    }
  }
  await visit(join(root, 'web/dist'));
  out['/wide.vtr'] = {type: 'application/octet-stream', body: await readFile(join(dir, 'wide.vtr'))};
  out['/values.fst'] = {type: 'application/octet-stream', body: await readFile(join(root, '../volna-core/tests/fixtures/values.fst'))};
  return out;
}
async function snapshot(b) {
  await b.evaluate(`window.profile = null; window.module.profile_tracks('state'); true`);
  return until(() => b.evaluate('window.profile'), 'hierarchy diagnostic');
}
async function stop(child) {
  if (!child.pid || child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise(resolve => child.once('exit', resolve));
  child.stdin.destroy();
  child.kill('SIGTERM');
  const force = setTimeout(() => child.kill('SIGKILL'), 2000);
  await exited;
  clearTimeout(force);
}

test('WASM local and remote hierarchies agree across 65536-entry pages', {timeout: 180000}, async t => {
  await stat(serverBinary);
  await stat(fixtureBinary);
  const dir = await mkdtemp(join(tmpdir(), 'volna-hierarchy-'));
  t.after(() => rm(dir, {recursive: true, force: true}));
  await promisify(execFile)(fixtureBinary, [join(dir, 'wide.vtr')], {timeout: 30000});
  const b = await browserTest(await routes(dir), {graphics: true, ready: `!!document.querySelector('canvas')`, readyTimeout: 60000});
  t.after(() => b.close());
  await b.evaluate(`(async () => {
    window.module = await import('./dist/volna.js');
    if (!window.module.profile_tracks) throw new Error('Build with VOLNA_WEB_FEATURES=remote-profile');
    window.profile = null;
    window.volnaProfileResult = text => { window.profile = JSON.parse(text); };
    window.outgoing = [];
    window.volnaTraceStart = connection => { window.connection = connection; };
    window.volnaTraceStop = () => {};
    window.volnaTraceSend = (_, bytes) => { window.outgoing.push(Array.from(bytes)); };
    window.yields = 0; window.volnaTraceYield = connection => { window.yields++; setTimeout(() => window.module.trace_continue(connection), 0); };
    window.metadata = name => JSON.stringify({traceUri: new URL('/' + name, location.href).href, candidates: {
      sidecar: {target: {kind: 'file', uri: new URL('/' + name + '.volna.json', location.href).href}, content: {status: 'missing'}, writable: false},
      fallback: {target: {kind: 'storage', key: name}, content: {status: 'missing'}, writable: false}}});
    return true;
  })()`);
  for (const [name, path] of [['wide.vtr', join(dir, 'wide.vtr')], ['values.fst', join(root, '../volna-core/tests/fixtures/values.fst')]]) {
    await b.evaluate(`(async () => { const bytes = new Uint8Array(await (await fetch('/${name}')).arrayBuffer());
      window.tryOpen = () => { try { window.module.open_resource('${name}', bytes, window.metadata('${name}')); return true; }
        catch (e) { if (String(e).includes('not ready')) return false; throw e; } }; return true; })()`);
    await until(() => b.evaluate('window.tryOpen()'), 'viewer host initialized');
    const local = await until(async () => {
      const s = await snapshot(b);
      return s.hierarchies.length === 1 && s.hierarchies[0].roots.every(r => r.size) ? s.hierarchies[0] : null;
    }, 'complete local hierarchy and sizes');
    if (name === 'wide.vtr') {
      assert.equal(local.scopes, 65538);
      assert.equal(local.vars, 65538);
      assert.deepEqual(local.roots[0].size, {signals: 1, variables: 65538, scopes: 65538});
      assert.equal(local.lastScope[0], 'cell65536');
      assert.equal(local.lastVar[0], '\\pin.λ');
    }
    const child = spawn(serverBinary, [path], {stdio: ['pipe', 'pipe', 'pipe']});
    let buffer = Buffer.alloc(0), stderr = '', processError;
    const frames = [];
    child.on('error', error => { processError = error; });
    child.stderr.on('data', bytes => { stderr += bytes; });
    child.stdout.on('data', bytes => {
      buffer = Buffer.concat([buffer, bytes]);
      while (buffer.length >= 12) {
        const length = 12 + buffer.readUInt32LE(8);
        assert.ok(length <= 1024 * 1024 + 12, 'bounded production frame');
        if (buffer.length < length) break;
        const frame = buffer.subarray(0, length);
        assert.equal(frame.readUInt32LE(4), 5);
        frames.push(frame);
        buffer = buffer.subarray(length);
      }
    });
    try {
      await b.evaluate(`window.outgoing = []; window.module.open_remote('${name}', window.metadata('${name}')); true`);
      let delivered = 0;
      let lastState;
      const remote = await until(async () => {
        if (processError) throw processError;
        if (child.exitCode !== null) throw new Error(`Server exited: ${stderr}`);
        const outgoing = await b.evaluate('window.outgoing.splice(0)');
        for (const bytes of outgoing) child.stdin.write(Buffer.from(bytes));
        for (const frame of frames.splice(0)) {
          delivered++;
          await b.evaluate(`window.module.trace_frame(window.connection, Uint8Array.from(atob('${frame.toString('base64')}'), c => c.charCodeAt(0))); true`);
        }
        if (delivered < 2) { await delay(5); return null; }
        const s = lastState = await snapshot(b);
        return s.hierarchies.length === 1 && s.hierarchies[0].roots.every(r => r.size) ? s.hierarchies[0] : null;
      }, 'complete remote hierarchy and server sizes', 60000).catch(async error => {
        throw new Error(`${error.message}; delivered=${delivered}; buffered=${buffer.length}; stderr=${stderr}; state=${JSON.stringify(lastState)}; host=${JSON.stringify(await b.evaluate('({connection:window.connection,yields:window.yields,outgoing:window.outgoing.length})'))}; exceptions=${JSON.stringify(b.exceptions)}`);
      });
      assert.deepEqual(remote, local);
      assert.ok(delivered >= (name === 'wide.vtr' ? 14 : 8), 'catalog and all page envelopes traveled');
    } finally { await stop(child); }
  }
  assert.deepEqual(b.exceptions, []);
});
