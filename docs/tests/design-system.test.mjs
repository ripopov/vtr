// node --test --test-concurrency=1 docs/tests/design-system.test.mjs
// The design system references the viewer's fonts, icons and app icon in place
// instead of copying them. These checks keep those references resolving and
// the WOFF2 faces loading; the browser test owns a headless Chrome and a
// loopback server over the repository.
import assert from 'node:assert/strict';
import {readdir, readFile, stat} from 'node:fs/promises';
import {dirname, join, normalize, relative, sep} from 'node:path';
import {test} from 'node:test';
import {fileURLToPath} from 'node:url';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';

const root = fileURLToPath(new URL('../..', import.meta.url));
const ds = join(root, 'docs/design-system');
const TYPES = {'.html': 'text/html', '.css': 'text/css', '.js': 'text/javascript', '.svg': 'image/svg+xml',
  '.png': 'image/png', '.woff2': 'font/woff2', '.ttf': 'font/ttf'};

async function files(dir) {
  const out = [];
  for (const e of await readdir(dir, {withFileTypes: true})) {
    const path = join(dir, e.name);
    if (e.isDirectory()) out.push(...await files(path));
    else out.push(path);
  }
  return out;
}

test('every relative asset reference in the design system resolves to a file', async () => {
  const sources = (await files(ds)).filter(f => /\.(html|css|kit|jsx)$/.test(f));
  const pattern = /["'(]((?:\.\.?\/)+[\w./-]+\.(?:svg|png|webp|woff2|ttf|css|js|kit|html))["')]/g;
  const missing = [];
  let checked = 0;
  for (const file of sources) {
    for (const [, ref] of (await readFile(file, 'utf8')).matchAll(pattern)) {
      checked++;
      const target = normalize(join(dirname(file), ref));
      if (!target.startsWith(root)) { missing.push(`${relative(root, file)}: ${ref} leaves the repository`); continue; }
      try { await stat(target); } catch { missing.push(`${relative(root, file)}: ${ref}`); }
    }
  }
  assert.ok(checked > 20, `found ${checked} references`);
  assert.deepEqual(missing, []);
});

test('fonts, icons and the app icon are referenced, not copied', async () => {
  const all = (await files(ds)).map(f => relative(ds, f));
  assert.deepEqual(all.filter(f => /\.ttf$|^assets[/\\](icons|logo)[/\\]/.test(f)), []);
  for (const face of ['Inter-Regular', 'Inter-SemiBold', 'JetBrainsMono-Regular']) {
    const bytes = await readFile(join(ds, 'fonts', face + '.woff2'));
    assert.equal(bytes.subarray(0, 4).toString('latin1'), 'wOF2', face);
  }
});

function repository() {
  return new Proxy({}, {get(_, path) {
    if (typeof path !== 'string') return undefined;
    if (path === '/') return {type: 'text/html', body: '<!doctype html><title>root</title>'};
    const file = normalize(join(root, decodeURIComponent(path)));
    if (!file.startsWith(root) || file.endsWith(sep)) return undefined;
    return {type: TYPES[file.slice(file.lastIndexOf('.'))] ?? 'application/octet-stream', body: () => readFile(file)};
  }});
}

test('cards load the WOFF2 faces and the viewer\'s own icons and app icon', {timeout: 60000}, async t => {
  const b = await browserTest(repository(), {ready: `location.protocol === 'http:'`});
  t.after(() => b.close());
  const origin = await b.evaluate('location.origin');
  const visit = async page => {
    await b.send('Page.navigate', {url: `${origin}/docs/design-system/${page}`});
    await b.wait(`document.readyState === 'complete' && location.pathname.endsWith('${page}')`);
  };

  await visit('guidelines/type-mono.html');
  const fonts = await b.evaluate(`Promise.all([...document.fonts].map(f => f.load())).then(faces => ({
    faces: faces.map(f => f.family.replace(/"/g, '') + ' ' + f.weight + ' ' + f.status).sort(),
    requested: performance.getEntriesByType('resource').map(e => e.name.split('/').pop()).filter(n => /\\.(woff2|ttf)$/.test(n)).sort()}))`);
  assert.deepEqual(fonts.faces, ['Inter 400 loaded', 'Inter 600 loaded', 'JetBrains Mono 400 loaded']);
  assert.deepEqual(fonts.requested, ['Inter-Regular.woff2', 'Inter-SemiBold.woff2', 'JetBrainsMono-Regular.woff2']);

  for (const page of ['guidelines/brand-logo.html', 'guidelines/brand-icons.html']) {
    await visit(page);
    const images = await b.evaluate(`Promise.all([...document.images].map(i => i.decode().then(() => null, () => i.getAttribute('src'))))
      .then(broken => ({count: document.images.length, broken: broken.filter(Boolean)}))`);
    assert.ok(images.count > 0, page);
    assert.deepEqual(images.broken, [], page);
  }
  assert.deepEqual(b.exceptions, []);
});
