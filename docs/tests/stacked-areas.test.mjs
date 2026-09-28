// node --test docs/tests/stacked-areas.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../stacked-areas.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const state = b => b.evaluate('SA.state()');
const group = (b, id) => b.evaluate(`SA.group(${JSON.stringify(id)})`);
const point = (b, fn, ...args) => b.evaluate(`SA.${fn}(${args.map(a => JSON.stringify(a)).join(',')})`);
const settled = b => b.wait('!SA.state().animating');
async function mouse(b, type, p, extra = {}) {
  await b.send('Input.dispatchMouseEvent', {type, x: p.x, y: p.y, button: 'left', clickCount: 1, ...extra});
}
async function click(b, p, extra = {}) {
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  await mouse(b, 'mousePressed', p, extra);
  await mouse(b, 'mouseReleased', p, extra);
}
const SHIFT = 8, CTRL = 2;
async function key(b, key, code, modifiers = 0, keyCode = 0) {
  for (const type of ['rawKeyDown', 'keyUp'])
    await b.send('Input.dispatchKeyEvent', {type, key, code, modifiers, windowsVirtualKeyCode: keyCode, text: type === 'rawKeyDown' && key.length === 1 && !(modifiers & CTRL) ? key : undefined});
}
async function guide(b, name) { await b.click(`[data-guide="${name}"]`); await settled(b); }
async function open(width = 1280) {
  const b = await browserTest(routes, {readyTimeout: 20000});
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 1000, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.evaluate('document.getElementById("pv").scrollIntoView({block: "start", behavior: "instant"})');
  // Let fonts and the scrolled layout settle before reading coordinates.
  await b.evaluate('document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))))');
  await settled(b);
  return b;
}
const near = (a, b, eps = 1e-9) => Math.abs(a - b) <= eps;

test('embedded data matches the parent proposal', async () => {
  const parent = await readFile(new URL('../c910-perf-counters.html', import.meta.url), 'utf8');
  const literal = (text, what) => {
    const line = text.split('\n').find(l => l.startsWith('const DATA = '));
    assert.ok(line, `${what} has a DATA line`);
    return JSON.parse(line.slice('const DATA = '.length).replace(/;\s*$/, ''));
  };
  const P = literal(parent, 'c910-perf-counters.html'), S = literal(html, 'stacked-areas.html');
  assert.equal(S.window, P.window); assert.equal(S.cycles, P.cycles); assert.equal(S.alphabet, P.alphabet);
  assert.equal(S.issued, P.totals.issued);
  for (const [k, v] of Object.entries(S.win)) assert.deepEqual(v, P.win[k], `win.${k}`);
  const K = {setup: 's', list: 'l', matrix: 'm', state: 't', crc: 'c', report: 'r'};
  assert.equal(S.kernel, P.win_kernel.map(k => K[k]).join(''));
  assert.equal(S.details.length, P.details.length);
  S.details.forEach((d, i) => {
    assert.equal(d.name, P.details[i].name); assert.equal(d.from, P.details[i].from_cycle);
    for (const [k, v] of Object.entries(d.series)) assert.equal(v, P.details[i].series[k], `${d.name}.${k}`);
  });
});

for (const width of [1280, 390]) test(`page layout, self-test and accessibility at ${width}px`, {timeout: 40000}, async t => {
  const b = await open(width); t.after(() => b.close());
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /selftest: all passed/, selftest);
  assert.doesNotMatch(selftest, /FAIL/);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links resolve');
  const unnamed = await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`);
  assert.equal(unnamed, 0, 'buttons have names');
  assert.equal(await b.evaluate('document.querySelectorAll(".callout,.card,.badge").length'), 0, 'no callout boxes');
  assert.equal(await b.evaluate('document.querySelectorAll("table").length <= 2'), true, 'at most two tables');
  assert.equal(await b.evaluate('document.getElementById("wbody").clientWidth > 300'), true, 'waves panel is visible');
  assert.equal(await b.evaluate('document.getElementById("wbody").scrollHeight <= document.getElementById("wbody").clientHeight + 1'), true, 'rows fit the panel');
  // Figures: stacked means stay under the real peak; stacked per-layer maxima rise above it.
  const f = await b.evaluate('SA.figure()');
  assert.ok(f.meanTop <= f.maxPeak + 1e-9, `means stay under the peak (${f.meanTop} ≤ ${f.maxPeak})`);
  assert.ok(f.maxTop > f.maxPeak && f.worst > f.worstPeak, `stacked maxima overshoot (${f.worst} vs ${f.worstPeak})`);
  assert.ok(f.over >= f.width / 4, `${f.over} of ${f.width} columns overshoot`);
  assert.equal(await b.evaluate('document.getElementById("fig-num").textContent'), `${f.over} of ${f.width}`);
  // The opening view: a stacked issue group over the whole run.
  const g = await group(b, 'issue');
  assert.equal(g.draw, 'stack'); assert.equal(g.mode, 'columns'); assert.equal(g.drawn, 'columns');
  assert.deepEqual(g.order.at(-1), 'issue.pipe0', 'pipe0 on the baseline');
  assert.deepEqual(b.exceptions, []);
});

test('guided scenarios: read, zoom, baseline, hide, partition, refusal', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());

  await guide(b, 'read');
  let s = await state(b), g = await group(b, 'issue');
  assert.equal(s.view, 'matrix'); assert.equal(s.cursor, 95232 + 612);
  assert.equal(g.total, 5, 'five pipes issue at the cursor');
  assert.ok(near(Object.values(g.values).reduce((a, v) => a + v, 0), g.total), 'the parts sum to the total');
  assert.equal(await point(b, 'value', 'issue'), 'Σ 5');
  assert.equal(await point(b, 'value', 'issue.pipe0'), '1');
  assert.equal(g.mode, 'columns', 'a fitted stretch is drawn as column means');
  assert.match(s.status, /column means/);

  await guide(b, 'cycles');
  g = await group(b, 'issue');
  assert.equal(g.mode, 'exact'); assert.equal(g.drawn, 'exact');
  assert.match((await state(b)).status, /exact steps/);
  // Rendered output: a cycle where pipe0 issues is painted in pipe0's colour at its layer.
  const tq = await b.evaluate(`(() => { for (let t = 95232 + 590; t < 95232 + 660; t++) { const r = SA.readoutAt('issue', t); if (r.values['issue.pipe0'] === 1 && t !== SA.state().cursor) return t; } })()`);
  assert.ok(tq, 'a cycle where pipe0 issues');
  const p0 = await point(b, 'plotPoint', 'issue', tq, 'issue.pipe0');
  assert.equal(await point(b, 'pixel', p0), await point(b, 'color', 'issue.pipe0'));

  await guide(b, 'baseline');
  g = await group(b, 'issue');
  assert.equal(g.order.at(-1), 'issue.pipe3', 'the load pipe is on the baseline');
  s = await state(b);
  assert.deepEqual(s.undo, ['Move issue.pipe3 to baseline']);
  assert.match(s.status, /Undo: Move issue.pipe3 to baseline/);

  await guide(b, 'hide');
  g = await group(b, 'issue');
  assert.deepEqual(g.hidden.sort(), ['issue.pipe6', 'issue.pipe7']);
  const r = await point(b, 'readoutAt', 'issue', 16640 + 300);
  assert.equal(r.of, 6); assert.equal(r.n, 8);
  assert.equal((await state(b)).undo.length, 3);

  await guide(b, 'slots');
  g = await group(b, 'topdown');
  assert.equal(g.draw, 'stack'); assert.equal(g.range, 'capacity'); assert.equal(g.top, 4);
  assert.equal(g.partition, true, 'the declared partition holds');
  assert.ok(near(g.total, 4), `slots sum to 4 at the cursor (${g.total})`);
  assert.ok((await state(b)).rows.includes('topdown.retiring'), 'the legend is unfolded');

  await guide(b, 'refuse');
  s = await state(b);
  assert.equal(s.menu, true);
  assert.match(s.menuText, /Stacked area needs one unit: rob.entries is entries, bus.rd_bw is B\/cycle/);
  assert.equal(await b.evaluate('document.querySelector(".menu [data-draw=stack]").disabled'), true);
  await key(b, 'Escape', 'Escape', 0, 27);
  assert.equal((await state(b)).menu, false);
  await key(b, 'A', 'KeyA', SHIFT, 65);
  s = await state(b);
  assert.match(s.notice, /needs one unit/, 'Shift+A explains the refusal');
  assert.equal((await group(b, 'mixed')).draw, 'activity');

  await guide(b, 'reset');
  s = await state(b);
  assert.deepEqual(s.undo, []); assert.equal(s.view, 'run');
  assert.deepEqual(b.exceptions, []);
});

test('pointer and keyboard: menus, legend, hover, zoom, undo', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.click('[data-view="matrix"]'); await settled(b);

  // The badge menu offers Draw, Range and Peak; Visible window eases to its target.
  await click(b, await point(b, 'badgePoint', 'issue'));
  let s = await state(b);
  assert.equal(s.menu, true);
  assert.match(s.menuText, /Activity.*Stacked area.*Capacity 8.*Whole trace.*Visible window.*Peak of total/);
  await b.click('.menu [data-range="window"]');
  await settled(b);
  let g = await group(b, 'issue');
  assert.equal(g.range, 'window'); assert.equal(g.top, g.target);
  const full = g.top;
  // Zoom into a quiet stretch with the keyboard: the window range follows.
  await click(b, {x: await point(b, 'timeX', 95232 + 40), y: (await point(b, 'plotPoint', 'issue', 95232 + 40)).y});
  for (let i = 0; i < 4; i++) await key(b, '=', 'Equal', 0, 187);
  await settled(b);
  g = await group(b, 'issue');
  assert.equal(g.mode, 'exact', 'zoomed in to single cycles');
  assert.ok(g.top <= full, `window range follows the view (${g.top} ≤ ${full})`);
  await key(b, 'f', 'KeyF', 0, 70);
  await settled(b);
  assert.equal((await group(b, 'issue')).mode, 'columns', 'F fits the stretch again');

  // Draw as activity through the menu, then undo and redo with the keyboard.
  await click(b, await point(b, 'badgePoint', 'issue'));
  await b.click('.menu [data-draw="activity"]');
  assert.equal((await group(b, 'issue')).draw, 'activity');
  await key(b, 'z', 'KeyZ', CTRL, 90);
  assert.equal((await group(b, 'issue')).draw, 'stack');
  assert.match((await state(b)).notice, /Undid: Draw issue as activity/);
  await key(b, 'z', 'KeyZ', CTRL | SHIFT, 90);
  assert.equal((await group(b, 'issue')).draw, 'activity');
  await key(b, 'z', 'KeyZ', CTRL, 90);

  // A legend swatch hides and shows a layer; the colour stays with the signal.
  await click(b, await point(b, 'swatchPoint', 'issue.pipe0'));
  g = await group(b, 'issue');
  assert.deepEqual(g.hidden, ['issue.pipe0']);
  assert.match((await state(b)).status, /Undo: Hide issue.pipe0/);
  await click(b, await point(b, 'swatchPoint', 'issue.pipe0'));
  assert.deepEqual((await group(b, 'issue')).hidden, []);

  // Right-click a legend row: Move to top.
  const name = await point(b, 'namePoint', 'issue.pipe2');
  await mouse(b, 'mouseMoved', name, {button: 'none'});
  await mouse(b, 'mousePressed', name, {button: 'right'});
  await mouse(b, 'mouseReleased', name, {button: 'right'});
  s = await state(b);
  assert.match(s.menuText, /Hide from stack.*Move to baseline.*Move to top/);
  await b.click('.menu [data-layer="top"]');
  assert.equal((await group(b, 'issue')).order[0], 'issue.pipe2');
  assert.deepEqual((await state(b)).undo.at(-1), 'Move issue.pipe2 to top');

  // Hover reads every part at the pointer, top to bottom, and highlights the layer under it.
  await b.evaluate('SA.state()');
  for (let i = 0; i < 4; i++) await key(b, '=', 'Equal', 0, 187);
  await settled(b);
  const vp = (await state(b)).vp;
  const tq = await b.evaluate(`(() => { for (let t = ${Math.ceil(vp[0]) + 2}; t < ${Math.floor(vp[1]) - 2}; t++) { const r = SA.readoutAt('issue', t); if (r.values['issue.pipe1'] === 1 && r.total >= 2 && t !== SA.state().cursor) return t; } })()`);
  assert.ok(tq, 'a cycle where pipe1 issues with another pipe');
  const hp = await point(b, 'plotPoint', 'issue', tq, 'issue.pipe1');
  await mouse(b, 'mouseMoved', hp, {button: 'none'});
  s = await state(b);
  assert.equal(s.hoverLayer, 'issue.pipe1');
  assert.match(s.tip, new RegExp(`^cycle ${tq.toLocaleString('en-US')}`));
  const order = (await group(b, 'issue')).order.map(sig => sig.replace('issue.', ''));
  const seen = [...s.tip.matchAll(/pipe(\d)/g)].map(m => 'pipe' + m[1]);
  assert.deepEqual(seen, order, 'the readout lists the layers top to bottom');
  assert.match(s.tip, /Σ\s*\d+ instr\/cycle/);
  // Other layers are dimmed while one is hovered.
  const other = await b.evaluate(`(() => { const g = SA.group('issue'); for (const sig of g.order) { if (sig === 'issue.pipe1') continue; const v = SA.readoutAt('issue', ${tq}).values[sig]; if (v === 1) return sig; } })()`);
  assert.ok(other, 'another layer issues in the same cycle');
  const op = await point(b, 'plotPoint', 'issue', tq, other);
  assert.notEqual(await point(b, 'pixel', op), await point(b, 'color', other), 'non-hovered layer is dimmed');
  assert.equal(await point(b, 'pixel', {x: hp.x, y: hp.y}), await point(b, 'color', 'issue.pipe1'), 'hovered layer keeps its colour');

  // Shift+→ steps the cursor to the next change of the total.
  await click(b, await point(b, 'namePoint', 'issue'));
  await click(b, {x: await point(b, 'timeX', tq), y: hp.y});
  const before = (await state(b)).cursor;
  await key(b, 'ArrowRight', 'ArrowRight', SHIFT, 39);
  const after = (await state(b)).cursor;
  assert.ok(after > before, 'the cursor moved right');
  const [ra, rb] = [await point(b, 'readoutAt', 'issue', after - 1), await point(b, 'readoutAt', 'issue', after)];
  assert.notEqual(ra.total, rb.total, 'it stopped where the total changes');

  // ← folds a selected group and → unfolds it; Shift+A toggles the stack.
  await click(b, await point(b, 'namePoint', 'issue'));
  await key(b, 'ArrowLeft', 'ArrowLeft', 0, 37);
  assert.equal((await state(b)).rows.includes('issue.pipe0'), false, 'folded');
  assert.equal((await group(b, 'issue')).draw, 'stack', 'a folded stack still draws');
  await key(b, 'ArrowRight', 'ArrowRight', 0, 39);
  assert.equal((await state(b)).rows.includes('issue.pipe0'), true, 'unfolded');
  await key(b, 'A', 'KeyA', SHIFT, 65);
  assert.equal((await group(b, 'issue')).draw, 'activity');
  await key(b, 'A', 'KeyA', SHIFT, 65);
  assert.equal((await group(b, 'issue')).draw, 'stack');
  // Folding and zooming are navigation: they made no undo steps.
  assert.ok((await state(b)).undo.every(l => !/fold|zoom/i.test(l)));
  assert.deepEqual(b.exceptions, []);
});
