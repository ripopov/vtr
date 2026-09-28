// node --test --test-concurrency=1 docs/tests/design-system-visual.test.mjs
// UPDATE_BASELINES=1 rewrites docs/design-system/baselines after an intended
// change; review the PNG and JSON diffs before committing them. Failing
// screenshot comparisons write current, baseline and diff images to
// DESIGN_DIFF_DIR (default: a temporary directory named in the failure).
import assert from 'node:assert/strict';
import {mkdir, mkdtemp, readFile, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, relative} from 'node:path';
import {test} from 'node:test';
import {adoptingPages, browser, ds, root} from './design-system-lib.mjs';

const update = process.env.UPDATE_BASELINES === '1';
const baselines = join(ds, 'baselines');
const gallery = join(ds, 'gallery.html');
const WIDTH = 1280;
/** A pixel differs when a channel moves by more than this; antialiasing noise stays below it. */
const CHANNEL = 48;
/** Share of differing pixels a section may have before it fails. */
const AREA = 0.002;

/** Computed styles that carry the design, one line per element, per section and theme. */
const SNAPSHOT = String.raw`(() => {
  // Sizes are left to the screenshots; inherited properties are recorded where they change.
  const PROPS = ['display', 'color', 'background-color', 'background-image', 'border-top', 'border-right', 'border-bottom', 'border-left',
    'border-radius', 'box-shadow', 'opacity', 'font-family', 'font-size', 'font-weight', 'line-height', 'letter-spacing',
    'text-transform', 'padding', 'margin', 'gap'];
  const INHERITED = new Set(['color', 'font-family', 'font-size', 'font-weight', 'line-height', 'letter-spacing', 'text-transform']);
  const DEFAULT = /^(rgba\(0, 0, 0, 0\)|none|0px|0px none .*|normal|1)$/;
  const read = (s, p) => {
    let v = s.getPropertyValue(p);
    if (p === 'font-family') v = v.split(',')[0].replace(/"/g, '');
    if (p === 'background-image') v = v.replace(/url\("[^"]*\/([^"/]+)"\)/g, 'url($1)');
    return v;
  };
  const out = {};
  for (const section of document.querySelectorAll('[data-gallery]')) {
    const lines = [];
    for (const pane of section.querySelectorAll(':scope > .g-theme')) {
      const walk = (el, path) => {
        const s = getComputedStyle(el), parent = getComputedStyle(el.parentElement);
        const values = PROPS.filter(p => INHERITED.has(p) ? read(s, p) !== read(parent, p) : !DEFAULT.test(read(s, p)))
          .map(p => p + ':' + read(s, p));
        lines.push(pane.dataset.theme + ' ' + path + ' ' + el.tagName.toLowerCase() + [...el.classList].map(c => '.' + c).join('') + ' {' + values.join('; ') + '}');
        [...el.children].forEach((c, i) => walk(c, path + '.' + i));
      };
      [...pane.children].forEach((c, i) => walk(c, String(i)));
    }
    out[section.id] = lines;
  }
  return out;
})()`;

/** Compares two PNG data URLs in the page; returns counts and, on failure, a diff image. */
const COMPARE = String.raw`(async (a, b, channel, area) => {
  const load = src => new Promise((ok, fail) => { const i = new Image(); i.onload = () => ok(i); i.onerror = fail; i.src = src; });
  const [x, y] = await Promise.all([load(a), load(b)]);
  if (x.width !== y.width || x.height !== y.height) return {size: [x.width, x.height, y.width, y.height]};
  const pixels = img => { const c = new OffscreenCanvas(img.width, img.height), g = c.getContext('2d'); g.drawImage(img, 0, 0);
    return g.getImageData(0, 0, img.width, img.height); };
  const p = pixels(x), q = pixels(y), d = new ImageData(x.width, x.height);
  let differing = 0;
  for (let i = 0; i < p.data.length; i += 4) {
    const delta = Math.max(Math.abs(p.data[i] - q.data[i]), Math.abs(p.data[i + 1] - q.data[i + 1]), Math.abs(p.data[i + 2] - q.data[i + 2]));
    const hit = delta > channel;
    if (hit) differing++;
    d.data.set(hit ? [255, 0, 80, 255] : [p.data[i] * .3, p.data[i + 1] * .3, p.data[i + 2] * .3, 255], i);
  }
  const share = differing / (x.width * x.height);
  if (share <= area) return {share};
  const c = new OffscreenCanvas(x.width, x.height); c.getContext('2d').putImageData(d, 0, 0);
  const blob = await c.convertToBlob({type: 'image/png'});
  const diff = await new Promise(r => { const f = new FileReader(); f.onload = () => r(f.result); f.readAsDataURL(blob); });
  return {share, diff};
})`;

async function readOr(file, fallback) {
  try { return await readFile(file, 'utf8'); } catch { return fallback; }
}

test('gallery computed styles match the baseline', {timeout: 120000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(gallery, {width: WIDTH, reducedMotion: true});
  const snapshot = await b.evaluate(SNAPSHOT);
  const file = join(baselines, 'styles.json');
  if (update) {
    await mkdir(baselines, {recursive: true});
    await writeFile(file, JSON.stringify(snapshot, null, 1) + '\n');
    return;
  }
  const expected = JSON.parse(await readOr(file, '{}'));
  assert.deepEqual(Object.keys(snapshot), Object.keys(expected), 'gallery sections; run with UPDATE_BASELINES=1 after adding one');
  const changes = [];
  for (const [section, lines] of Object.entries(snapshot)) {
    const before = expected[section];
    if (lines.length !== before.length) { changes.push(`${section}: ${before.length} elements became ${lines.length}`); continue; }
    lines.forEach((line, i) => { if (line !== before[i]) changes.push(`${section}\n  - ${before[i]}\n  + ${line}`); });
  }
  assert.equal(changes.length, 0, `${changes.length} style changes (UPDATE_BASELINES=1 accepts intended ones):\n${changes.slice(0, 12).join('\n')}`);
});

test('gallery sections match their screenshots', {timeout: 180000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(gallery, {width: WIDTH, reducedMotion: true});
  const sections = await b.evaluate(`[...document.querySelectorAll('[data-gallery]')].map(s => { const r = s.getBoundingClientRect();
    return {id: s.id, x: 0, y: Math.round(r.top + scrollY), width: ${WIDTH}, height: Math.round(r.height)}; })`);
  // Product screenshots are content, not design: keep their boxes, drop their pixels.
  await b.evaluate(`document.head.insertAdjacentHTML('beforeend', '<style>.g-theme img{visibility:hidden}</style>')`);
  let out;
  const failures = [];
  for (const {id, ...clip} of sections) {
    const {data} = await b.send('Page.captureScreenshot', {format: 'png', clip: {...clip, scale: 1}, captureBeyondViewport: true});
    const file = join(baselines, `${id}.png`);
    if (update) { await writeFile(file, Buffer.from(data, 'base64')); continue; }
    let baseline;
    try { baseline = (await readFile(file)).toString('base64'); } catch { failures.push(`${id}: no baseline; run with UPDATE_BASELINES=1`); continue; }
    const result = await b.evaluate(`${COMPARE}('data:image/png;base64,${data}', 'data:image/png;base64,${baseline}', ${CHANNEL}, ${AREA})`);
    if (!result.size && !result.diff) continue;
    out ??= process.env.DESIGN_DIFF_DIR || await mkdtemp(join(tmpdir(), 'volna-design-diff-'));
    await mkdir(out, {recursive: true});
    await writeFile(join(out, `${id}.current.png`), Buffer.from(data, 'base64'));
    await writeFile(join(out, `${id}.baseline.png`), Buffer.from(baseline, 'base64'));
    if (result.diff) await writeFile(join(out, `${id}.diff.png`), Buffer.from(result.diff.split(',')[1], 'base64'));
    failures.push(result.size ? `${id}: size ${result.size[0]}×${result.size[1]}, baseline ${result.size[2]}×${result.size[3]}`
      : `${id}: ${(result.share * 100).toFixed(2)}% of pixels differ`);
  }
  assert.deepEqual(failures, [], out && `images in ${out}`);
  assert.deepEqual(b.exceptions, []);
});

test('adopting pages fit a 360px phone without horizontal scrolling', {timeout: 120000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  const wide = [];
  for (const page of await adoptingPages()) {
    await b.open(page, {width: 360, height: 740});
    const v = await b.evaluate(`({scroll: document.documentElement.scrollWidth, client: document.documentElement.clientWidth,
      culprits: [...document.body.querySelectorAll('*')].filter(e => { const r = e.getBoundingClientRect();
        return r.width && r.right > document.documentElement.clientWidth + 0.5 && !e.closest('.v-table-wrap,.v-tabs__list,pre,.v-visually-hidden'); })
        .slice(0, 5).map(e => e.tagName.toLowerCase() + [...e.classList].map(c => '.' + c).join('') + ' ' + Math.round(e.getBoundingClientRect().right))})`);
    if (v.scroll > v.client) wide.push(`${relative(root, page)}: ${v.scroll}px wide at ${v.client}px; ${v.culprits.join(', ')}`);
  }
  assert.deepEqual(wide, []);
});

test('every keyboard stop shows the focus ring', {timeout: 120000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(gallery, {width: WIDTH});
  const missing = [];
  let stops = 0;
  for (let i = 0; i < 400; i++) {
    for (const type of ['rawKeyDown', 'keyUp']) await b.send('Input.dispatchKeyEvent', {type, key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9});
    const v = await b.evaluate(`(() => { const e = document.activeElement; if (!e || e === document.body) return null;
      const s = getComputedStyle(e); return {name: e.tagName.toLowerCase() + [...e.classList].map(c => '.' + c).join(''), text: e.textContent.trim().slice(0, 24),
        visible: e.matches(':focus-visible'), ok: s.outlineStyle !== 'none' && parseFloat(s.outlineWidth) >= 2}; })()`);
    if (!v) break;
    stops++;
    if (!v.visible || !v.ok) missing.push(`${v.name} "${v.text}"`);
  }
  assert.ok(stops > 40, `${stops} keyboard stops`);
  assert.deepEqual(missing, []);
});

test('reduced motion stops every transition and animation', {timeout: 60000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  const moving = `[...document.querySelectorAll('*')].filter(e => { const s = getComputedStyle(e);
    return [s.transitionDuration, s.animationDuration].some(d => d.split(',').some(v => parseFloat(v) > 0)); })
    .map(e => e.tagName.toLowerCase() + [...e.classList].map(c => '.' + c).join(''))`;
  await b.open(gallery, {width: WIDTH});
  assert.ok((await b.evaluate(moving)).includes('button.v-btn.v-btn--primary'), 'buttons animate by default');
  await b.open(gallery, {width: WIDTH, reducedMotion: true});
  assert.deepEqual(await b.evaluate(moving), []);
});
