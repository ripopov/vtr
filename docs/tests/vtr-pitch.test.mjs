// node --test docs/tests/vtr-pitch.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../vtr-pitch.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};
const SENTENCES = [
  'An AI skill reads your RTL and generates monitors with automated checks.',
  'VTR records transactions and runtime links; VDB gives them design meaning.',
  'Volna makes stalls and bottlenecks visible in an interactive view of your design.',
];

async function open(width = 1280, height = 900) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.evaluate('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))');
  return b;
}

test('one slide: headline, one described diagram, captions per level, embedded fonts', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  const r = await b.evaluate(`(() => { const s = document.querySelector('.slide'); return {
    slides: document.querySelectorAll('.slide').length, header: s.querySelectorAll('.bar').length, title: s.querySelector('h1').textContent, sub: s.querySelector('.sub').textContent,
    svgs: s.querySelectorAll('svg').length, role: s.querySelector('svg.dg').getAttribute('role'),
    label: s.querySelector('svg.dg').getAttribute('aria-label'),
    tags: [...s.querySelectorAll('.copy .tag')].map(e => e.textContent),
    sentences: [...s.querySelectorAll('.copy p.s')].map(p => p.textContent) }; })()`);
  assert.equal(r.slides, 1);
  assert.equal(r.header, 0, 'no header bar above the headline');
  assert.equal(r.title, 'Level up your traces');
  assert.equal(r.sub, 'See your hardware run at the level you designed it.');
  assert.equal(r.svgs, 1);
  assert.equal(r.role, 'img');
  for (const word of ['RTL', 'top.sv', 'monitor', 'VTR', 'VDB', 'pipeline', 'sequence', 'bandwidth']) assert.ok(r.label.includes(word), `the description mentions ${word}`);
  assert.deepEqual(r.tags, ['L0 · Input', 'L1 · AI skill', 'L2 · VTR + VDB', 'L3 · Volna']);
  assert.deepEqual(r.sentences, SENTENCES);
  const fonts = await b.evaluate(`[...document.fonts].map(f => f.family.replace(/"/g, '') + ' ' + f.weight + ' ' + f.status)`);
  assert.deepEqual(fonts.sort(), ['Lilex 400 loaded', 'Plex 400 loaded', 'Plex 600 loaded']);
  assert.ok(!/(src|href)="https?:/.test(html), 'no network resources');
  assert.deepEqual(b.exceptions, []);
});

test('text stays inside the margins, inside its boxes and apart', {timeout: 30000}, async t => {
  const b = await open(1280, 800); t.after(() => b.close());
  const problems = await b.evaluate(`(() => {
    const s = document.querySelector('.slide'), f = s.getBoundingClientRect(), k = f.width / 1280;
    const box = q => { const r = q.getBoundingClientRect(); return {x: (r.left - f.left) / k, y: (r.top - f.top) / k, w: r.width / k, h: r.height / k}; };
    const texts = [];
    s.querySelectorAll('h1, .sub, .copy p, .tag').forEach(e => {
      const range = document.createRange(); range.selectNodeContents(e);
      for (const r of range.getClientRects()) texts.push({s: e.textContent.slice(0, 30), e, x: (r.left - f.left) / k, y: (r.top - f.top) / k, w: r.width / k, h: r.height / k});
    });
    const svg = s.querySelector('svg.dg');
    svg.querySelectorAll('text').forEach(e => texts.push({s: e.textContent, svg: true, ...box(e)}));
    const rects = [...svg.querySelectorAll('rect[data-box]')].map(box);
    const out = [];
    for (const t of texts) {
      if (t.x < 71.5 || t.y < 39.5 || t.x + t.w > 1208.5 || t.y + t.h > 684) out.push('outside the margins: ' + t.s);
      if (!t.svg) continue;
      const cx = t.x + t.w / 2, cy = t.y + t.h / 2;
      const inside = rects.filter(q => cx > q.x && cx < q.x + q.w && cy > q.y && cy < q.y + q.h).sort((a, c) => a.w * a.h - c.w * c.h)[0];
      if (inside && (t.x < inside.x + 3 || t.x + t.w > inside.x + inside.w - 3)) out.push('touches its box edge: ' + t.s);
    }
    for (let i = 0; i < texts.length; i++) for (let j = i + 1; j < texts.length; j++) {
      const a = texts[i], c = texts[j];
      if (a.e && a.e === c.e) continue;
      const ox = Math.min(a.x + a.w, c.x + c.w) - Math.max(a.x, c.x), oy = Math.min(a.y + a.h, c.y + c.h) - Math.max(a.y, c.y);
      if (ox > 0.5 && oy > 2) out.push('overlap: ' + a.s + ' / ' + c.s);
    }
    return out; })()`);
  assert.deepEqual(problems, []);
});

test('everything sits on the staircase grid', {timeout: 30000}, async t => {
  const b = await open(1280, 800); t.after(() => b.close());
  const g = await b.evaluate(`(() => {
    const s = document.querySelector('.slide'), f = s.getBoundingClientRect(), k = f.width / 1280;
    const bb = sel => [...s.querySelectorAll('svg.dg ' + sel)].map(e => e.getBBox());
    return {
      items: [...s.querySelectorAll('.copy li')].map(e => { const r = e.getBoundingClientRect(); return [(r.left - f.left) / k, (r.top - f.top) / k]; }),
      treads: [...s.querySelectorAll('svg.dg .ln-tread')].map(e => { const r = e.getBoundingClientRect(), q = e.getBBox(); return {x: (r.left - f.left) / k, y: (r.top - f.top) / k + 1, bx: q.x, by: q.y, w: q.width}; }),
      panels: bb('.f-panel').map(r => [r.x, r.y + r.height, r.width]),
      sizes: bb('.f-panel').map(r => [r.width, r.height]),
      bars: bb('.f-bw, .f-bw-stall').map(r => [r.x, r.width]),
      dips: bb('.f-bw-stall').map(r => r.x),
      stalls: [...new Set(bb('.f-warn').filter(r => r.height === 12).map(r => r.x))] };
  })()`);
  assert.deepEqual(g.treads.map(t => t.by), [488, 436, 384, 332], 'a gentle 52px rise per step');
  g.items.forEach(([x, y], n) => {
    assert.ok(Math.abs(x - (g.treads[n].x + 16)) < 0.5, `level ${n} caption is inset 16px like its panel (${x})`);
    assert.ok(Math.abs(y - (g.treads[n].y + 20)) < 1.5, `level ${n} caption is 20px under its tread (${y})`);
  });
  const onStep = n => [g.treads[n].bx + 16, g.treads[n].by - 16, g.treads[n].w - 32];
  assert.deepEqual(g.treads.map(t => t.w), [284, 284, 284, 284], 'equal tread widths');
  assert.deepEqual(g.panels, [0, 1, 2, 3].map(onStep), 'one centered panel per step with equal insets');
  assert.deepEqual(g.sizes, Array.from({length: 4}, () => [252, 176]), 'equal diagram footprints');
  assert.equal(g.bars.length, 9);
  assert.deepEqual(g.dips, g.stalls, 'the abstract views preserve the shared stall');
});

for (const [width, height] of [[1920, 1080], [1280, 800], [1024, 768]]) test(`the slide keeps 16:9 and fits the window at ${width}px`, {timeout: 30000}, async t => {
  const b = await open(width, height); t.after(() => b.close());
  const r = await b.evaluate(`(() => { const q = document.querySelector('.slide').getBoundingClientRect(), n = document.querySelector('.ctl').getBoundingClientRect(); return {w: q.width, h: q.height, bottom: q.bottom, right: q.right, nav: n.top}; })()`);
  assert.ok(Math.abs(r.w / r.h - 16 / 9) < 0.01, JSON.stringify(r));
  assert.ok(r.right <= width && r.bottom <= r.nav + 1, `slide and controls both visible: ${JSON.stringify(r)}`);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth && document.documentElement.scrollHeight <= innerHeight'), true);
});

test('phone width: a readable document, the diagram scrolls inside its frame', {timeout: 30000}, async t => {
  const b = await open(390, 844); t.after(() => b.close());
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const r = await b.evaluate(`(() => { const s = document.querySelector('.slide');
    return {font: Math.min(...[...s.querySelectorAll('.copy p')].map(p => parseFloat(getComputedStyle(p).fontSize))),
      svg: s.querySelector('svg.dg').getBoundingClientRect().width, overflow: getComputedStyle(s.querySelector('.fig')).overflowX,
      right: Math.max(...[...s.querySelectorAll('h1, .sub, .copy p')].map(e => e.getBoundingClientRect().right))}; })()`);
  assert.ok(r.font >= 16 && r.svg >= 700 && r.overflow === 'auto' && r.right <= 390 - 16, JSON.stringify(r));
  assert.deepEqual(b.exceptions, []);
});

test('P and the button present full screen; Escape leaves', {timeout: 30000}, async t => {
  const b = await open(1280, 720); t.after(() => b.close());
  const key = async (key, code, vk) => { for (const type of ['rawKeyDown', 'keyUp']) await b.send('Input.dispatchKeyEvent', {type, key, code, windowsVirtualKeyCode: vk}); };
  await key('p', 'KeyP', 80);
  await b.wait('document.documentElement.classList.contains("present")');
  await b.evaluate('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))');
  const r = await b.evaluate(`(() => { const q = document.getElementById('pitch').getBoundingClientRect(); return {w: q.width, h: q.height, x: q.left, y: q.top, nav: getComputedStyle(document.querySelector('.ctl')).display}; })()`);
  assert.equal(r.nav, 'none');
  assert.ok(Math.abs(r.w - 1280) < 1 && Math.abs(r.h - 720) < 1 && Math.abs(r.x) < 1 && Math.abs(r.y) < 1, JSON.stringify(r));
  await key('Escape', 'Escape', 27);
  await b.wait('!document.documentElement.classList.contains("present")');
  await b.evaluate('document.querySelector("[data-present]").click()');
  await b.wait('document.documentElement.classList.contains("present")');
  assert.deepEqual(b.exceptions, []);
});

test('text contrast on every surface it is drawn on', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  const pairs = [['ink', 'bg'], ['mut', 'bg'], ['acc-ink', 'bg'], ['ink', 'step-top'], ['mut', 'step-top'], ['acc-ink', 'step-top'],
    ['ink', 'panel'], ['mut', 'panel'], ['acc-ink', 'panel'], ['ok', 'panel'], ['warn-ink', 'panel'], ['ink', 'chip'],
    ['acc-ink', 'acc-soft'], ['ink', 'warn-soft'], ['on-acc', 'acc'], ['on-warn', 'warn']];
  const r = await b.evaluate(`(() => {
    const lum = h => { const v = [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16) / 255).map(x => x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4); return 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]; };
    const cs = getComputedStyle(document.querySelector('.slide')), hex = n => cs.getPropertyValue('--' + n).trim(), out = {};
    for (const [fg, bg] of ${JSON.stringify(pairs)}) { const [x, y] = [lum(hex(fg)), lum(hex(bg))].sort((p, q) => q - p); out[fg + ' on ' + bg] = (x + 0.05) / (y + 0.05); }
    return out; })()`);
  for (const [name, v] of Object.entries(r)) assert.ok(v >= 4.5, `${name}: ${v.toFixed(2)}`);
});
