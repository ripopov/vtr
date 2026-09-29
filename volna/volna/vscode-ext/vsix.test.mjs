// Installs the actual VSIX into isolated VS Code, opens VTR/FST through their
// default association, and asserts real WASM/server startup. Linux uses Xvfb;
// this runner owns all processes, profiles and fixture copies (Node >= 22).
import assert from 'node:assert/strict';
import {spawn, execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {access, copyFile, mkdir, mkdtemp, readFile, rm, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {dirname, join, resolve} from 'node:path';
import {createServer} from 'node:net';
import {fileURLToPath} from 'node:url';
import {test} from 'node:test';
import {until} from '../tools/browser-test.mjs';

const ext = dirname(fileURLToPath(import.meta.url));
const exec = promisify(execFile);

async function freePort() {
  const server = createServer();
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}

async function connect(url) {
  const ws = new WebSocket(url);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  const pending = new Map(), contexts = new Map(), logs = [];
  let next = 0;
  const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const id = ++next;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 15000);
    pending.set(id, {resolve, reject, timer});
    ws.send(JSON.stringify({id, method, params, sessionId}));
  });
  const enable = async sessionId => {
    await send('Runtime.enable', {}, sessionId);
    await send('Log.enable', {}, sessionId);
    await send('Target.setAutoAttach', {autoAttach: true, waitForDebuggerOnStart: false, flatten: true}, sessionId);
  };
  ws.onmessage = ({data}) => {
    const m = JSON.parse(data), p = m.params, sessionId = m.sessionId;
    const request = pending.get(m.id);
    if (request) {
      clearTimeout(request.timer); pending.delete(m.id);
      if (m.error) request.reject(new Error(JSON.stringify(m.error)));
      else request.resolve(m.result);
    } else if (m.method === 'Target.attachedToTarget') {
      enable(p.sessionId).catch(error => logs.push(String(error)));
    } else if (m.method === 'Runtime.executionContextCreated' && p.context.auxData?.isDefault) {
      contexts.set(`${sessionId}:${p.context.id}`, {id: p.context.id, sessionId});
    } else if (m.method === 'Runtime.executionContextDestroyed') {
      contexts.delete(`${sessionId}:${p.executionContextId}`);
    } else if (m.method === 'Runtime.executionContextsCleared' || m.method === 'Target.detachedFromTarget') {
      const cleared = m.method === 'Target.detachedFromTarget' ? p.sessionId : sessionId;
      for (const [key, value] of contexts) if (value.sessionId === cleared) contexts.delete(key);
    } else if (m.method === 'Runtime.exceptionThrown') {
      logs.push(JSON.stringify(p.exceptionDetails));
    } else if (m.method === 'Runtime.consoleAPICalled') {
      if (p.type === 'error' || p.type === 'warning') logs.push(`[${p.type}] ${p.args.map(arg => arg.value ?? arg.description ?? '').join(' ')}`);
    } else if (m.method === 'Log.entryAdded' && p.entry.level === 'error') {
      logs.push(p.entry.text);
    }
  };
  await enable();
  await send('Page.bringToFront');
  return {
    logs,
    async viewer() {
      for (const context of contexts.values()) {
        try {
          const found = await this.evaluate(`Array.from(document.scripts).some(s => s.textContent.includes('volna.js'))`, context);
          if (found) return context;
        } catch {} // Navigating/closed frames disappear between CDP events.
      }
    },
    async evaluate(expression, context) {
      const result = await send('Runtime.evaluate', {expression, contextId: context?.id, awaitPromise: true, returnByValue: true}, context?.sessionId);
      if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
      return result.result.value;
    },
    close() {
      ws.close();
      for (const request of pending.values()) { clearTimeout(request.timer); request.reject(new Error('VS Code closed')); }
      pending.clear();
    },
  };
}

test('installed VSIX opens and renders VTR and FST with its bundled server', {timeout: 180000}, async t => {
  assert.equal(process.platform, 'linux', 'this headless host requires Linux and xvfb-run');
  const version = JSON.parse(await readFile(join(ext, 'package.json'), 'utf8')).version;
  const target = `linux-${process.arch}`;
  const vsix = resolve(process.env.VOLNA_VSIX ?? join(ext, `volna-${version}-${target}.vsix`));
  await access(vsix).catch(() => { throw new Error(`Missing ${vsix}; run package_vsix.py first`); });
  const root = await mkdtemp(join(tmpdir(), 'volna-vsix-test-'));
  let child, cdp, output = '', spawnError;
  async function hostExited() {
    const detail = await readFile(join(root, 'driver-error.txt'), 'utf8').catch(() => '');
    return new Error(`VS Code exited (${child.exitCode}): ${detail}\n${output}`);
  }
  t.after(async () => {
    cdp?.close();
    if (child && child.exitCode === null && child.signalCode === null && !spawnError) {
      const exited = new Promise(resolve => child.once('exit', resolve));
      process.kill(-child.pid, 'SIGTERM');
      const force = setTimeout(() => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} }, 2000);
      await exited; clearTimeout(force);
    }
    await rm(root, {recursive: true, force: true, maxRetries: 5});
  });
  const code = process.env.VOLNA_CODE ?? 'code';
  const profile = join(root, 'profile'), extensions = join(root, 'extensions'), probe = join(root, 'probe');
  await mkdir(join(profile, 'User'), {recursive: true});
  await mkdir(probe);
  await writeFile(join(profile, 'User', 'settings.json'), JSON.stringify({
    'workbench.startupEditor': 'none', 'window.restoreWindows': 'none',
    'security.workspace.trust.enabled': false, 'volna.workspace.autosave': 'off',
    'update.mode': 'none', 'extensions.autoUpdate': false,
  }));
  await writeFile(join(probe, 'package.json'), JSON.stringify({
    name: 'volna-vsix-probe', version: '0.0.0', publisher: 'vtr-test', engines: {vscode: '^1.90.0'},
    main: './probe.cjs',
  }));
  await writeFile(join(probe, 'probe.cjs'), 'exports.activate = () => {};');
  await copyFile(join(ext, 'vsix-driver.cjs'), join(probe, 'run.cjs'));
  await copyFile(join(ext, '..', 'examples', 'picorv32.vtr'), join(root, 'picorv32.vtr'));
  await copyFile(join(ext, '..', '..', 'volna-core', 'tests', 'fixtures', 'values.fst'), join(root, 'values.fst'));
  await exec(code, ['--user-data-dir', profile, '--extensions-dir', extensions, '--install-extension', vsix, '--force'], {timeout: 30000});
  const port = await freePort();
  child = spawn('xvfb-run', ['-a', code, '--new-window', '--wait', '--user-data-dir', profile,
    '--extensions-dir', extensions, `--extensionDevelopmentPath=${probe}`, `--extensionTestsPath=${join(probe, 'run.cjs')}`,
    '--skip-welcome', '--skip-release-notes', '--disable-workspace-trust',
    `--remote-debugging-port=${port}`, '--enable-unsafe-swiftshader', '--use-angle=swiftshader',
    '--enable-unsafe-webgpu', '--use-webgpu-adapter=swiftshader', root],
  {detached: true, env: {...process.env, VOLNA_VSIX_TEST_DIR: root}, stdio: ['ignore', 'pipe', 'pipe']});
  child.stdout.on('data', bytes => { output = (output + bytes).slice(-12000); });
  child.stderr.on('data', bytes => { output = (output + bytes).slice(-12000); });
  child.on('error', error => { spawnError = error; });
  const targetPage = await until(async () => {
    if (spawnError) throw spawnError;
    if (child.exitCode !== null) throw await hostExited();
    try {
      return (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(p => p.type === 'page');
    } catch { return null; }
  }, 'VS Code debugging endpoint', 30000);
  cdp = await connect(targetPage.webSocketDebuggerUrl);
  for (const [index, name] of ['picorv32.vtr', 'values.fst'].entries()) {
    await until(async () => {
      if (child.exitCode !== null) throw await hostExited();
      try { return JSON.parse(await readFile(join(root, 'current.json'), 'utf8')).index === index; } catch { return false; }
    }, `default custom editor for ${name}`, 30000);
    let lastState, context;
    try {
      context = await until(() => cdp.viewer(), `${name} webview`, 10000);
      await until(() => cdp.evaluate(`!!document.querySelector('canvas')`, context), `${name} viewer canvas`, 60000);
      await cdp.evaluate(`import(Array.from(document.scripts).map(s => s.textContent).join('').match(/from "([^"]+volna\\.js)"/)[1]).then(m => { window.volnaTestModule = m; })`, context);
      let openedWaves = false;
      const state = await until(async () => {
        const state = await cdp.evaluate(`new Promise(resolve => {
          const original = console.info;
          const timer = setTimeout(() => { console.info = original; resolve(null); }, 1000);
          console.info = (...args) => {
            original(...args); const text = args.join(' ');
            if (text.includes('STATE ')) { clearTimeout(timer); console.info = original; resolve(text); }
          };
          window.volnaTestModule.debug_state();
        })`, context);
        lastState = state;
        // A trace opens on the start panel. Use the normal host command to
        // create waves, whose initial viewport derives from trace metadata.
        const events = (await readFile(join(root, 'trace-events.jsonl'), 'utf8').catch(() => '')).trim().split('\n').filter(Boolean).map(line => JSON.parse(line));
        const activeEvents = events.filter(e => e.name === name);
        const failure = activeEvents.find(e => e.type === 'traceError');
        if (failure) throw new Error(failure.message);
        if (state?.includes('STATE ') && !openedWaves && activeEvents.filter(e => e.type === 'traceRequest').length >= 4) {
          await cdp.evaluate(`window.volnaTestModule.dispatch_command('newPanel')`, context);
          openedWaves = true;
          return null;
        }
        const viewport = name === 'picorv32.vtr' ? 'viewport=(0,10000000)' : 'viewport=(5,20)';
        return state?.includes(viewport) && /frames .*total=[1-9]\d*/.test(state) ? state : null;
      }, `${name} trace metadata loaded through bundled server`, 30000);
      assert.match(state, /frames .*total=[1-9]\d*/, 'viewer painted frames');
      console.log(`${name}: ${state}`);
      await writeFile(join(root, `passed-${index}`), 'ok');
    } catch (error) {
      const hostText = await cdp.evaluate('document.body.innerText').catch(() => '');
      const events = await readFile(join(root, 'trace-events.jsonl'), 'utf8').catch(() => '');
      throw new Error(`${error.message}\nLast viewer state:\n${lastState}\nHost events:\n${events}\nWebview errors:\n${cdp.logs.join('\n')}\nVS Code:\n${output}\nHost UI:\n${hostText}`, {cause: error});
    }
  }
  await until(() => child.exitCode !== null, 'VS Code test host shutdown', 15000);
  assert.equal(child.exitCode, 0, output);
  await access(join(root, 'done'));
});
