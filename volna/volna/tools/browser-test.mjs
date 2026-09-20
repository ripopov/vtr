// Self-contained Node >= 22 browser test host. No running browser/server needed.
import {spawn} from 'node:child_process';
import {access, mkdtemp, readFile, rm, writeFile} from 'node:fs/promises';
import {createServer} from 'node:http';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';

export async function until(fn, description, timeout = 10000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await fn();
    if (value) return value;
    await delay(20);
  }
  throw new Error(`Timed out: ${description}`);
}
export async function browserTest(routes, {graphics = false, url, ready = 'window.ready === true', readyTimeout = 10000} = {}) {
  const candidates = [process.env.VOLNA_CHROME,
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome',
  ].filter(Boolean);
  let binary;
  for (const candidate of candidates) {
    try { await access(candidate); binary = candidate; break; } catch {}
  }
  if (!binary) throw new Error('Chrome/Chromium is required; set VOLNA_CHROME to its executable.');
  const profile = await mkdtemp(join(tmpdir(), 'volna-browser-test-'));
  const server = createServer(async (req, res) => {
    try {
      const route = routes[new URL(req.url, 'http://localhost').pathname];
      if (!route) { res.writeHead(404); res.end(); return; }
      res.setHeader('Content-Type', route.type);
      res.end(typeof route.body === 'function' ? await route.body() : route.body);
    } catch (error) { res.writeHead(500); res.end(String(error)); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const child = spawn(binary, ['--headless=new', `--user-data-dir=${profile}`,
    '--remote-debugging-port=0', '--no-first-run', '--no-default-browser-check',
    '--window-size=1000,700', '--force-device-scale-factor=1',
    ...(graphics ? ['--enable-unsafe-swiftshader', '--use-angle=swiftshader',
      '--enable-unsafe-webgpu', '--use-webgpu-adapter=swiftshader'] : []), 'about:blank'],
  {stdio: ['ignore', 'ignore', 'pipe']});
  let stderr = '', spawnError;
  child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-10000); });
  child.on('error', error => { spawnError = error; });
  let ws;
  const pending = new Map();
  const exceptions = [];
  const contexts = new Map();
  async function close() {
    ws?.close();
    for (const request of pending.values()) { clearTimeout(request.timer); request.reject(new Error('Browser closed')); }
    pending.clear();
    if (child.exitCode === null && child.signalCode === null && !spawnError) {
      const exited = new Promise(resolve => child.once('exit', resolve));
      child.kill('SIGTERM');
      const force = setTimeout(() => child.kill('SIGKILL'), 2000);
      await exited;
      clearTimeout(force);
    }
    await new Promise(resolve => server.close(resolve));
    await rm(profile, {recursive: true, force: true, maxRetries: 3});
  }
  try {
    const port = await until(async () => {
      if (spawnError) throw spawnError;
      if (child.exitCode !== null) throw new Error(`Chrome exited: ${stderr}`);
      try { return (await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; }
      catch { return null; }
    }, 'Chrome debugging endpoint');
    const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    const page = targets.find(t => t.type === 'page');
    ws = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
    let id = 0;
    ws.onmessage = ({data}) => {
      const message = JSON.parse(data);
      const key = id => `${message.sessionId ?? ''}:${id}`;
      if (message.method === 'Runtime.executionContextCreated') {
        const context = message.params.context;
        contexts.set(key(context.id), {...context, sessionId:message.sessionId});
      } else if (message.method === 'Runtime.executionContextDestroyed') {
        contexts.delete(key(message.params.executionContextId));
      } else if (message.method === 'Runtime.executionContextsCleared') {
        for (const [key,context] of contexts) if (context.sessionId === message.sessionId) contexts.delete(key);
      }
      const request = pending.get(message.id);
      if (request) {
        clearTimeout(request.timer); pending.delete(message.id);
        if (message.error) request.reject(new Error(JSON.stringify(message.error)));
        else request.resolve(message.result);
      } else if (message.method === 'Runtime.exceptionThrown') {
        exceptions.push(message.params.exceptionDetails);
      }
    };
    const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
      const requestId = ++id;
      const timer = setTimeout(() => {
        pending.delete(requestId); reject(new Error(`CDP timed out: ${method}`));
      }, 15000);
      pending.set(requestId, {resolve, reject, timer});
      ws.send(JSON.stringify({id: requestId, method, params, sessionId}));
    });
    const evaluate = async expression => {
      const result = await send('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true, userGesture: false});
      if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
      return result.result.value;
    };
    await send('Runtime.enable');
    await send('Page.enable');
    await send('Browser.grantPermissions', {origin, permissions: ['clipboardReadWrite', 'clipboardSanitizedWrite']});
    await send('Page.navigate', {url: url ?? origin});
    await until(() => evaluate(ready), 'test fixture ready', readyTimeout);
    await send('Page.bringToFront');
    await until(() => evaluate('document.hasFocus()'), 'browser document focus');
    return {
      close, send, evaluate, exceptions, contexts,
      wait: expression => until(() => evaluate(expression), expression),
      click: async selector => {
        const point = await evaluate(`(() => {
          const e = document.querySelector(${JSON.stringify(selector)});
          if (!e) throw new Error('Missing element');
          e.scrollIntoView({block:'center',inline:'nearest',behavior:'instant'});
          const r = e.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2};
        })()`);
        await send('Input.dispatchMouseEvent', {type:'mousePressed', button:'left', clickCount:1, ...point});
        await send('Input.dispatchMouseEvent', {type:'mouseReleased', button:'left', clickCount:1, ...point});
      },
      screenshot: async path => {
        const {data} = await send('Page.captureScreenshot', {format:'png'});
        await writeFile(path, Buffer.from(data, 'base64'));
      },
    };
  } catch (error) { await close(); throw error; }
}
