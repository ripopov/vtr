// node --test docs/tests/vtr-pitch.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../vtr-pitch.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};
const SLIDES = ['level-up', 'down-up', 'one-file'];
const bench = JSON.parse(await readFile(new URL('../../bench/results/latest/results.json', import.meta.url), 'utf8'));
const settle = 'new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))';
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

test('three slides; the staircase: headline, one described diagram, captions per level, embedded fonts', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  assert.deepEqual(await b.evaluate(`[...document.querySelectorAll('.slide')].map(s => s.id)`), SLIDES);
  const r = await b.evaluate(`(() => { const s = document.getElementById('level-up'); return {
    header: s.querySelectorAll('.bar').length, title: s.querySelector('h1').textContent, sub: s.querySelector('.sub').textContent,
    svgs: s.querySelectorAll('svg').length, role: s.querySelector('svg.dg').getAttribute('role'),
    label: s.querySelector('svg.dg').getAttribute('aria-label'),
    tags: [...s.querySelectorAll('.copy .tag')].map(e => e.textContent),
    sentences: [...s.querySelectorAll('.copy p.s')].map(p => p.textContent) }; })()`);
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

test('two columns: design optimized down to RTL faces the trace lifted by agents, level by level', {timeout: 30000}, async t => {
  const b = await open(1280, 800); t.after(() => b.close());
  await b.evaluate(`PITCH.show(1); ${settle}`);
  const v = await b.evaluate(`(() => { const s = document.getElementById('down-up'), svg = s.querySelector('svg.dg');
    const card = e => { const r = e.getBBox(), t = [...svg.querySelectorAll('text.w6')].find(q => { const b = q.getBBox(); return b.x > r.x && b.x < r.x + r.width && b.y > r.y && b.y < r.y + r.height; }); return {x: r.x, y: r.y, w: r.width, h: r.height, title: t?.textContent}; };
    const arms = [...svg.querySelectorAll('path[marker-end]')].filter(e => { const r = e.getBBox(); return r.height > 200; }).map(e => e.getAttribute('d'));
    return {title: s.querySelector('h1').textContent, sub: s.querySelector('.sub').textContent, label: svg.getAttribute('aria-label'),
      left: [...svg.querySelectorAll('rect.f-panel')].map(card), right: [...svg.querySelectorAll('rect.f-lift')].map(card),
      chips: [...svg.querySelectorAll('rect.f-acc-soft')].map(e => { const r = e.getBBox(); return r.x + r.width / 2; }),
      band: (r => [r.x, r.width])(svg.querySelector('rect.f-band').getBBox()), width: svg.viewBox.baseVal.width, arms}; })()`);
  assert.equal(v.title, 'Optimized down. Lifted back up.');
  assert.equal(v.sub, 'Optimization pushes every design down to RTL, the only executable spec as detailed as silicon.');
  for (const word of ['optimization', 'executable specification', 'silicon', 'agents', 'RTL', 'waveforms', 'transactions', 'AI skill', 'VTR', 'VDB', 'Volna', 'concept space']) assert.ok(v.label.includes(word), `the description mentions ${word}`);
  assert.deepEqual(v.left.map(c => c.title), ['Architecture', 'Micro-architecture', 'Interfaces', 'RTL'], 'optimization pushes the design down from the top');
  assert.deepEqual(v.right.map(c => c.title), ['Flows, performance', 'Pipelines, causes', 'Transactions', 'Waveforms'], 'agents lift the trace back to the top');
  v.left.forEach((l, i) => {
    const r = v.right[i];
    assert.equal(l.y, r.y, `level ${i}: the lifted card faces its design card`);
    assert.equal(l.x + l.w, v.width - r.x, `level ${i}: the two arms mirror each other`);
    assert.deepEqual([l.w, l.h, r.w, r.h], [300, 72, 300, 72], 'equal cards');
    assert.deepEqual([l.x, r.x], [v.left[0].x, v.right[0].x], `level ${i}: both columns stay vertical`);
    if (i) assert.equal(l.y - v.left[i - 1].y, 100, 'levels are 100px apart');
  });
  for (const c of v.chips) assert.equal(c, v.width / 2, 'what sits between the arms is centred on the axis');
  assert.deepEqual(v.band, [0, v.width], 'the concept space spans both arms');
  assert.equal(v.arms.length, 2);
  const pts = d => d.match(/-?[\d.]+/g).map(Number);
  const [a, c] = v.arms.map(pts);
  assert.deepEqual([v.width - a[0], a[1], v.width - a[2], a[3]], [c[2], c[3], c[0], c[1]], 'the design arrow and the trace arrow are mirror images');
  assert.ok(a[3] > a[1] && c[3] < c[1], 'design goes down, the trace comes up');
  assert.ok(a[0] === a[2] && c[0] === c[2] && a[0] < v.left[0].x && c[0] > v.right[0].x + v.right[0].w, 'the arms run straight down the outer edges');
});

test('one file: FST and FTR feed VTR, and the benchmarks are the checked-in results drawn to scale', {timeout: 30000}, async t => {
  const b = await open(1280, 800); t.after(() => b.close());
  await b.evaluate(`PITCH.show(2); ${settle}`);
  const v = await b.evaluate(`(() => { const s = document.getElementById('one-file'), svg = s.querySelector('svg.dg');
    const texts = [...svg.querySelectorAll('text')];
    const titled = cls => [...svg.querySelectorAll(cls)].map(e => { const r = e.getBBox(), t = texts.find(q => { const b = q.getBBox(); return q.classList.contains('w6') && b.x > r.x && b.x < r.x + r.width && b.y > r.y && b.y < r.y + 40; }); return {x: r.x, y: r.y, w: r.width, h: r.height, title: t?.textContent}; });
    const bars = cls => [...svg.querySelectorAll(cls)].map(e => e.getBBox()).map(r => [r.x, r.y, r.width]);
    return {title: s.querySelector('h1').textContent, sub: s.querySelector('.sub').textContent, label: svg.getAttribute('aria-label'),
      formats: titled('rect.f-panel').filter(c => c.w === 236), vtr: titled('rect.f-lift'), them: bars('rect.f-them'), ours: bars('rect.f-vtr[data-box], rect.f-vtr:not(.hatch)'), saved: bars('rect.f-save'), tones: [...svg.querySelectorAll('rect.f-them')].map(e => e.getAttribute('class')),
      down: [...svg.querySelectorAll('path[marker-end]')].map(e => e.getBBox()).map(r => [r.x, r.y, r.height]),
      texts: texts.map(e => e.textContent)}; })()`);
  assert.equal(v.title, 'Two formats. One modern file.');
  assert.equal(v.sub, 'VTR packs the best ideas of FST waveforms and FTR transactions into one compressed format.');
  for (const word of ['FST', 'FTR', 'value chains', 'relations', 'one file', 'zstd', 'random access', 'crash', 'Benchmarks']) assert.ok(v.label.includes(word), `the description mentions ${word}`);
  assert.deepEqual(v.formats.map(c => c.title), ['FST', 'FTR']);
  const colours = await b.evaluate(`(() => { const s = document.getElementById('one-file'), fill = sel => getComputedStyle(s.querySelector(sel)).fill;
    return [fill('rect.f-them.f-fst'), fill('rect.f-them.f-ftr'), fill('rect.f-vtr')]; })()`);
  assert.equal(new Set(colours).size, 3, `VTR, FST and FTR each have their own colour: ${colours}`);
  assert.deepEqual(v.vtr.map(c => c.title), ['VTR']);
  const [fst, ftr] = v.formats, [vtr] = v.vtr;
  assert.equal(fst.y, ftr.y, 'the two formats sit side by side');
  assert.equal(fst.x, vtr.x, 'VTR spans both formats');
  assert.equal(ftr.x + ftr.w, vtr.x + vtr.w, 'VTR spans both formats');
  assert.ok(v.down.some(([x, y, h]) => Math.abs(x + 0.75 - (vtr.x + vtr.w / 2)) < 1 && y >= fst.y + fst.h && y + h <= vtr.y), 'one arrow feeds VTR from between the formats');

  // The rows, recomputed from the benchmark data: best FST or FTR variant per metric against VTR.
  const c910 = bench.rtl.find(r => r.workload === 'c910_coremark'), tlm = bench.tx.find(r => r.workload === 'tlm_1m');
  const fstW = ['fstapi-lz4', 'fstapi-zlib', 'fstcpp-lz4'].map(k => c910.writers[k]), ftrW = ['ftr-lz4', 'ftr-raw'].map(k => tlm.writers[k]);
  const rd = c910.readers, min = a => Math.min(...a);
  const rows = [
    [min(fstW.map(w => w.bytes)), c910.writers['vtr-rust'].bytes, 'fst', 'smaller file'],
    [min(fstW.map(w => w.wall_s)), c910.writers['vtr-rust'].wall_s, 'fst', 'faster write'],
    [min([rd.vs_fstapi_zlib.load_1.fst_wellen, rd.fstapi_reader_zlib.load_1_s, rd.fstapi_reader_lz4.load_1_s]), rd.vs_fstapi_zlib.load_1.vtr, 'fst', 'faster read'],
    [min(ftrW.map(w => w.bytes)), tlm.writers['vtr-rust'].bytes, 'ftr', 'smaller file'],
    [min(ftrW.map(w => w.wall_s)), tlm.writers['vtr-rust'].wall_s, 'ftr', 'faster write']];
  assert.equal(v.them.length, rows.length);
  assert.equal(v.ours.length, rows.length);
  assert.equal(v.saved.length, rows.length);
  const at = (s, i) => v.texts.indexOf(s, i);
  let k = -1;
  rows.forEach(([them, ours, tone, says], i) => {
    assert.ok(ours < them, `row ${i}: VTR wins`);
    k = at(`${(them / ours).toFixed(1)}×`, k + 1);
    assert.ok(k >= 0 && v.texts[k + 1] === says, `row ${i}: the ratio reads ${(them / ours).toFixed(1)}× ${says}`);
    assert.ok(v.tones[i].includes(`f-${tone}`), `row ${i}: the competitor bar has ${tone}'s colour`);
    const [tx, ty, tw] = v.them[i], [ox, oy, ow] = v.ours[i];
    assert.equal(tx, ox, `row ${i}: both bars start on one axis`);
    assert.equal(tw, v.them[0][2], `row ${i}: the competitor bar is full scale`);
    assert.ok(Math.abs(ow / tw - ours / them) < 0.001, `row ${i}: the VTR bar is drawn to scale (${ow}/${tw} vs ${ours}/${them})`);
    assert.equal(oy - ty, 18, `row ${i}: VTR under its competitor`);
    const [sx, , sw] = v.saved[i];
    assert.ok(sx < ox + ow && Math.abs(sx + sw - (tx + tw)) < 1, `row ${i}: the hatched saving runs from VTR's bar out to the competitor's length`);
  });
  assert.ok(v.label.includes(`${c910.info.signals.toLocaleString('en-US')} signals`), 'the description names the workload');
  for (const gone of ['BENCHMARKS', 'variant', 'workloads', 'FST + FTR']) assert.ok(!v.texts.some(s => s.includes(gone)), `no fine print: ${gone}`);
});

test('text stays inside the margins, inside its boxes and apart on every slide', {timeout: 30000}, async t => {
  const b = await open(1280, 800); t.after(() => b.close());
  for (const [n, id] of SLIDES.entries()) {
  await b.evaluate(`PITCH.show(${n}); ${settle}`);
  const problems = await b.evaluate(`(() => {
    const s = document.getElementById('${id}'), f = s.getBoundingClientRect(), k = f.width / 1280;
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
  assert.deepEqual(problems, [], id);
  }
});

test('diagram text paints where it is laid out, even after the canvas rescales on another slide', {timeout: 30000}, async t => {
  // The browser starts at 1000x700; resizing and switching in the same frame rescales slide 2 while it is hidden.
  const b = await browserTest(routes); t.after(() => b.close());
  await b.send('Emulation.setDeviceMetricsOverride', {width: 1440, height: 900, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  for (const n of [1, 2, 0]) {
    await b.evaluate(`PITCH.show(${n}); ${settle}`);
    const shot = (await b.send('Page.captureScreenshot', {format: 'png'})).data;
    const dark = await b.evaluate(`(async () => {
      const im = new Image(); im.src = 'data:image/png;base64,${shot}'; await im.decode();
      const c = document.createElement('canvas'); c.width = im.width; c.height = im.height; const g = c.getContext('2d'); g.drawImage(im, 0, 0);
      return [...document.querySelectorAll('#${SLIDES[n]} svg.dg text')].filter(e => {
        const r = e.getBoundingClientRect(), d = g.getImageData(r.x, r.y, Math.max(1, r.width), Math.max(1, r.height)).data;
        for (let i = 0; i < d.length; i += 4) if (d[i] + d[i + 1] + d[i + 2] > 300) return false;
        return true;
      }).map(e => e.textContent); })()`);
    assert.deepEqual(dark, [], `${SLIDES[n]}: no text box is left without painted text`);
  }
});

test('everything sits on the staircase grid', {timeout: 30000}, async t => {
  const b = await open(1280, 800); t.after(() => b.close());
  const g = await b.evaluate(`(() => {
    const s = document.getElementById('level-up'), f = s.getBoundingClientRect(), k = f.width / 1280;
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

for (const [width, height] of [[1920, 1080], [1280, 800], [1024, 768]]) test(`the slides keep 16:9 and fit the window at ${width}px`, {timeout: 30000}, async t => {
  const b = await open(width, height); t.after(() => b.close());
  for (const n of SLIDES.keys()) {
  await b.evaluate(`PITCH.show(${n}); ${settle}`);
  const r = await b.evaluate(`(() => { const q = document.querySelector('.slide:not([hidden])').getBoundingClientRect(), n = document.querySelector('.ctl').getBoundingClientRect(); return {w: q.width, h: q.height, bottom: q.bottom, right: q.right, nav: n.top}; })()`);
  assert.ok(Math.abs(r.w / r.h - 16 / 9) < 0.01, JSON.stringify(r));
  assert.ok(r.right <= width && r.bottom <= r.nav + 1, `slide and controls both visible: ${JSON.stringify(r)}`);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth && document.documentElement.scrollHeight <= innerHeight'), true);
  }
});

test('phone width: a readable document, the diagram scrolls inside its frame', {timeout: 30000}, async t => {
  const b = await open(390, 844); t.after(() => b.close());
  for (const n of SLIDES.keys()) {
  await b.evaluate(`PITCH.show(${n}); ${settle}`);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const r = await b.evaluate(`(() => { const s = document.querySelector('.slide:not([hidden])');
    return {font: Math.min(...[...s.querySelectorAll('.copy p, .sub')].map(p => parseFloat(getComputedStyle(p).fontSize))),
      svg: s.querySelector('svg.dg').getBoundingClientRect().width, overflow: getComputedStyle(s.querySelector('.fig')).overflowX,
      right: Math.max(...[...s.querySelectorAll('h1, .sub, .copy p')].map(e => e.getBoundingClientRect().right))}; })()`);
  assert.ok(r.font >= 16 && r.svg >= 700 && r.overflow === 'auto' && r.right <= 390 - 16, JSON.stringify(r));
  }
  assert.deepEqual(b.exceptions, []);
});

test('keys, buttons and links switch slides; P presents full screen; Escape leaves', {timeout: 30000}, async t => {
  const b = await open(1280, 720); t.after(() => b.close());
  const key = async (key, code, vk) => { for (const type of ['rawKeyDown', 'keyUp']) await b.send('Input.dispatchKeyEvent', {type, key, code, windowsVirtualKeyCode: vk}); };
  await key('ArrowRight', 'ArrowRight', 39);
  await b.wait('PITCH.current() === 1 && location.hash === "#down-up" && !document.getElementById("down-up").hidden && document.getElementById("level-up").hidden');
  assert.equal(await b.evaluate('document.querySelector(".ctl [aria-current=true]").textContent'), '2 Down and up');
  await key('ArrowLeft', 'ArrowLeft', 37);
  await b.wait('PITCH.current() === 0');
  await b.evaluate('document.querySelector("[data-go=\\"1\\"]").click()');
  await b.wait('PITCH.current() === 1 && document.getElementById("said").textContent.startsWith("2 ·")');
  await key('3', 'Digit3', 51);
  await b.wait('PITCH.current() === 2 && location.hash === "#one-file"');
  await key('ArrowRight', 'ArrowRight', 39);
  await b.wait('PITCH.current() === 2');
  await key('Home', 'Home', 36);
  await b.wait('PITCH.current() === 0');
  await key('End', 'End', 35);
  await b.wait('PITCH.current() === 2');
  await b.evaluate('location.hash = "#level-up"');
  await b.wait('PITCH.current() === 0');
  await key('p', 'KeyP', 80);
  await b.wait('document.documentElement.classList.contains("present")');
  await b.evaluate('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))');
  const r = await b.evaluate(`(() => { const q = document.getElementById('level-up').getBoundingClientRect(); return {w: q.width, h: q.height, x: q.left, y: q.top, nav: getComputedStyle(document.querySelector('.ctl')).display}; })()`);
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
    ['acc-ink', 'acc-soft'], ['ink', 'acc-soft'], ['mut', 'acc-soft'], ['ink', 'warn-soft'], ['on-acc', 'acc'], ['on-warn', 'warn'],
    ['ink', 'band'], ['mut', 'band'], ['acc-ink', 'band'], ['ink', 'acc2'], ['fst', 'panel'], ['ftr', 'panel'], ['vtr', 'panel'], ['vtr', 'acc-soft']];
  const r = await b.evaluate(`(() => {
    const lum = h => { const v = [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16) / 255).map(x => x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4); return 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]; };
    const cs = getComputedStyle(document.querySelector('.slide')), hex = n => cs.getPropertyValue('--' + n).trim(), out = {};
    for (const [fg, bg] of ${JSON.stringify(pairs)}) { const [x, y] = [lum(hex(fg)), lum(hex(bg))].sort((p, q) => q - p); out[fg + ' on ' + bg] = (x + 0.05) / (y + 0.05); }
    return out; })()`);
  for (const [name, v] of Object.entries(r)) assert.ok(v >= 4.5, `${name}: ${v.toFixed(2)}`);
});
