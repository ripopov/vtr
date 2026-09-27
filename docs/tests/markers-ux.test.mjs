// node --test docs/tests/markers-ux.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../markers-ux.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const ALT = 1, CTRL = 2, SHIFT = 8;
const state = b => b.evaluate('MK.state()');
const settled = b => b.wait('!MK.state().busy && !MK.state().animating');
async function open(width = 1280) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 900, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.evaluate('MK.speed = 8');
  return b;
}
async function mouse(b, type, p, extra = {}) {
  await b.send('Input.dispatchMouseEvent', {type, x: p.x, y: p.y, button: 'left', clickCount: 1, ...extra});
}
async function click(b, p, extra = {}) {
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  await mouse(b, 'mousePressed', p, extra);
  await mouse(b, 'mouseReleased', p, extra);
}
async function dblclick(b, p) {
  await click(b, p);
  await mouse(b, 'mousePressed', p, {clickCount: 2});
  await mouse(b, 'mouseReleased', p, {clickCount: 2});
}
async function key(b, key, modifiers = 0, code = '') {
  for (const type of ['rawKeyDown', 'keyUp'])
    await b.send('Input.dispatchKeyEvent', {type, key, code, modifiers});
}
async function typeText(b, text) { await b.send('Input.insertText', {text}); }
async function focusPanel(b) {
  await b.evaluate(`document.getElementById('pv').scrollIntoView({block: 'center', behavior: 'instant'}); document.getElementById('pv').focus()`);
  await b.evaluate('document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))))');
}
async function guide(b, name) { await b.click(`[data-guide="${name}"]`); await b.wait('MK.state().busy === false'); await settled(b); }

for (const width of [1280, 390]) test(`page layout, self-test and accessibility at ${width}px`, {timeout: 30000}, async t => {
  const b = await open(width); t.after(() => b.close());
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /selftest: all passed/, selftest);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links and citations resolve');
  const unnamed = await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`);
  assert.equal(unnamed, 0, 'buttons have names');
  assert.equal(await b.evaluate('document.querySelectorAll(".callout,.card,.badge").length'), 0, 'no callout boxes');
  assert.equal(await b.evaluate('document.querySelector("#pv canvas").clientWidth > 300'), true, 'wave panel is visible');
  const numbers = await b.evaluate(`[...document.querySelectorAll('[data-n]')].filter(e => !e.textContent.trim()).length`);
  assert.equal(numbers, 0, 'every quoted number is filled from the trace');
  assert.deepEqual(b.exceptions, []);
});

test('the trace and the measurements the prose quotes', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  // numbers come from the model; these are the ones the page's story depends on
  const m = await b.evaluate(`(() => { const E = MK.ev, c = MK.trace.clocks;
    const B = MEASURE.between(c, E.B.req, E.B.resp), A = MEASURE.between(c, E.A.req, E.A.resp);
    return {b: MEASURE.text(B), a: MEASURE.text(A, {sign: false}), nc: MEASURE.naive(c[0].tl, E.B.req, E.B.resp), na: MEASURE.naive(c[1].tl, E.B.req, E.B.resp)}; })()`);
  assert.equal(m.b, '+126 ns · +80 core_clk · +26.4 axi_clk');
  assert.equal(m.a, '37 ns · 37 core_clk · 14.8 axi_clk');
  assert.deepEqual([m.nc, m.na], [126, 50.4], 'division gives the wrong answers the page shows');
  const table = await b.evaluate(`document.getElementById('count-tbl').textContent`);
  assert.match(table, /core_clk126\s*6380/); assert.match(table, /axi_clk50\.4\s*50\.426\.4/);
  // gating: no cycles are counted while the AXI clock is stopped
  const g = await b.evaluate(`MEASURE.between([MK.trace.clocks[1]], MK.ev.gateStart, MK.ev.gateEnd - 1).clocks[0]`);
  assert.deepEqual(g, {name: 'axi_clk', cycles: 0, exact: true});
});

test('guided scenarios end in the states their text describes', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  const ev = await b.evaluate('MK.ev');

  await guide(b, 'bookmark');
  let s = await state(b);
  assert.deepEqual(s.markers.find(m => m.id === 4), {id: 4, time: ev.B.req, label: 'req B'}, 'marker 4 takes the lowest free number and the typed name');
  assert.equal(s.editor, null);
  assert.match(await b.evaluate('story.textContent'), /req_valid ↑/, 'the editor offered the selected row\'s change');

  await guide(b, 'measure');
  s = await state(b);
  assert.deepEqual(s.reference, {marker: 4}); assert.equal(s.cursor, ev.B.resp);
  assert.equal(s.live.full, '+126 ns · +80 core_clk · +26.4 axi_clk');
  assert.ok(s.view.a <= ev.B.req && s.view.b >= ev.B.resp, 'Z framed the measurement');
  assert.match(s.status, /R → cursor \+126 ns · \+80 core_clk · \+26\.4 axi_clk/);
  assert.ok(s.spans.some(sp => sp.from === 4 && sp.to === 3 && sp.label), 'the adjacent span is labelled');

  await guide(b, 'walk');
  s = await state(b);
  assert.equal(s.cursor, ev.irq, '` returned to where the digit jump started');
  assert.equal(s.back.cursor, ev.A.resp);

  await guide(b, 'drag');
  s = await state(b);
  assert.equal(s.markers.find(m => m.id === 4).time, ev.B.first, 'the dragged marker snapped onto the rvalid edge');
  await focusPanel(b); await key(b, 'z', CTRL);
  assert.equal((await state(b)).markers.find(m => m.id === 4).time, ev.B.first - 1300, 'undo restores the old position');

  await guide(b, 'crowd');
  s = await state(b);
  assert.match(await b.evaluate('story.textContent'), /zoomed to them/);
  const beats = s.chips.filter(c => c.kind === 'one' && /B beat/.test(c.text));
  assert.equal(beats.length, 4, 'after zooming into the cluster, every beat has its own labelled chip');

  await guide(b, 'undo');
  s = await state(b);
  assert.deepEqual(s.markers.find(m => m.id === 3), {id: 3, time: ev.irq, label: 'irq'}); assert.equal(s.redo, 1);

  await guide(b, 'find');
  s = await state(b);
  assert.equal(s.nav, true);
  assert.deepEqual(await b.evaluate(`[...document.querySelectorAll('.nav-row')].map(r => r.children[1].textContent)`), ['resp A', 'resp B']);
  assert.deepEqual(b.exceptions, []);
});

test('keys: mark, name, remove, undo, step, jump, back, reference, zoom', {timeout: 40000}, async t => {
  const b = await open(); t.after(() => b.close());
  const ev = await b.evaluate('MK.ev');
  await b.evaluate(`MK.base({cursor: MK.ev.A.req})`);
  await focusPanel(b);
  // M on an existing marker opens its name editor
  await key(b, 'm');
  let s = await state(b);
  assert.equal(s.editor, 1); assert.equal(s.markers.length, 3, 'M on a marker adds nothing');
  assert.equal(await b.evaluate('document.activeElement.value'), 'req A');
  await b.evaluate('document.activeElement.select()'); await typeText(b, 'first request'); await key(b, 'Enter');
  assert.equal((await state(b)).markers[0].label, 'first request');
  // Escape in the editor keeps the old name
  await key(b, 'm'); await typeText(b, 'discarded'); await key(b, 'Escape');
  assert.equal((await state(b)).markers[0].label, 'first request');
  // a new marker takes the lowest free number
  await b.evaluate('MK.app.setCursor(100000)'); await focusPanel(b);
  await key(b, 'm');
  s = await state(b); assert.ok(s.markers.some(m => m.id === 4 && m.time === 100000));
  // Shift+M removes one marker; Ctrl+Z restores it; Ctrl+Shift+Z removes it again
  await key(b, 'M', SHIFT);
  assert.equal((await state(b)).markers.some(m => m.id === 4), false);
  assert.match((await state(b)).notice, /Removed marker 4/);
  await key(b, 'z', CTRL); assert.equal((await state(b)).markers.some(m => m.id === 4), true);
  await key(b, 'Z', CTRL | SHIFT); assert.equal((await state(b)).markers.some(m => m.id === 4), false);
  await key(b, 'z', CTRL);
  // stepping and jumping
  await key(b, ','); await settled(b); assert.equal((await state(b)).cursor, ev.A.resp);
  await key(b, '.'); await settled(b); assert.equal((await state(b)).cursor, 100000);
  await key(b, '3'); await settled(b); assert.equal((await state(b)).cursor, ev.irq);
  await key(b, '`'); await settled(b); assert.equal((await state(b)).cursor, 100000, 'back to where the jump started');
  await key(b, '`'); await settled(b); assert.equal((await state(b)).cursor, ev.irq, 'and forward again');
  await key(b, '9'); assert.match((await state(b)).notice, /No marker 9/);
  // reference, live measurement, zoom to it, clear
  await key(b, '1'); await settled(b); await key(b, 'r');
  assert.deepEqual((await state(b)).reference, {marker: 1});
  await key(b, '3'); await settled(b);
  s = await state(b);
  assert.match(s.live.full, /^\+217 ns · \+/);
  await key(b, 'z'); await settled(b);
  s = await state(b); assert.ok(s.view.a <= ev.A.req && s.view.b >= ev.irq && s.view.b - s.view.a < 400000);
  await key(b, 'R', SHIFT); assert.equal((await state(b)).reference, null);
  // the reference follows its marker; removing the marker leaves it where the marker was
  await key(b, '2'); await settled(b); await key(b, 'r');
  await b.evaluate('MK.app.markers.move(2, 60000)');
  assert.equal((await state(b)).referenceTime, 60000);
  await b.evaluate('MK.app.removeMarker(2)');
  assert.deepEqual((await state(b)).reference, {time: 60000});
  await key(b, 'z', CTRL); assert.deepEqual((await state(b)).reference, {marker: 2}, 'undo re-attaches the reference');
  // the navigator: open, filter, go
  await key(b, "'"); assert.equal((await state(b)).nav, true);
  await typeText(b, 'irq'); await key(b, 'Enter'); await settled(b);
  s = await state(b); assert.equal(s.nav, false); assert.equal(s.cursor, ev.irq);
  assert.deepEqual(b.exceptions, []);
});

test('pointer: chips go, drag with snapping, cancel, rename, reference, menu, spans', {timeout: 40000}, async t => {
  const b = await open(); t.after(() => b.close());
  const ev = await b.evaluate('MK.ev');
  await b.evaluate(`MK.base({view: [0, 80000], cursor: 70000})`);
  await focusPanel(b);
  // click a chip: the cursor goes to the marker
  let p = await b.evaluate('MK.point("chip", 1)');
  await click(b, p);
  assert.equal((await state(b)).cursor, ev.A.req);
  // hover shows the marker's details
  p = await b.evaluate('MK.point("chip", 2)');
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  assert.match(await b.evaluate('document.querySelector(".pv-tip").textContent'), /Marker 2 · resp A/);
  // drag marker 2 about 30 px right: it lands on an edge of the selected clock (core_clk, 1 GHz here)
  await mouse(b, 'mousePressed', p);
  for (let k = 1; k <= 6; k++) await mouse(b, 'mouseMoved', {x: p.x + 5 * k, y: p.y});
  await mouse(b, 'mouseReleased', {x: p.x + 30, y: p.y});
  let s = await state(b);
  const moved = s.markers.find(m => m.id === 2).time;
  assert.ok(moved > ev.A.resp, 'moved right');
  assert.equal(await b.evaluate(`MK.trace.clocks[0].tl.cycleAt(${moved}).edge`), moved, 'snapped to a core_clk edge');
  await key(b, 'z', CTRL); assert.equal((await state(b)).markers.find(m => m.id === 2).time, ev.A.resp, 'one undo step');
  // Escape during a drag cancels it
  p = await b.evaluate('MK.point("chip", 2)');
  await mouse(b, 'mousePressed', p);
  for (let k = 1; k <= 4; k++) await mouse(b, 'mouseMoved', {x: p.x + 8 * k, y: p.y});
  await key(b, 'Escape');
  await mouse(b, 'mouseReleased', {x: p.x + 32, y: p.y});
  s = await state(b);
  assert.equal(s.markers.find(m => m.id === 2).time, ev.A.resp, 'Escape put it back'); assert.equal(s.undo, 0, 'a cancelled drag journals nothing');
  // double-click renames in place
  p = await b.evaluate('MK.point("chip", 2)');
  await dblclick(b, p);
  assert.equal((await state(b)).editor, 2);
  await b.evaluate('document.activeElement.select()'); await typeText(b, 'last beat seen'); await key(b, 'Enter');
  assert.equal((await state(b)).markers.find(m => m.id === 2).label, 'last beat seen');
  // Alt-click in the waves sets a free reference at the snapped point; middle-click on a chip attaches it
  const w = await b.evaluate(`MK.point("wave", {t: 40000, row: "lsu.stall"})`);
  const before = (await state(b)).cursor;
  await click(b, w, {modifiers: ALT});
  s = await state(b); assert.ok(s.reference && 'time' in s.reference); assert.equal(s.cursor, before, 'Alt-click leaves the cursor alone');
  assert.equal(await b.evaluate(`MK.trace.clocks[0].tl.cycleAt(${s.reference.time}).edge`), s.reference.time, 'the reference snapped to an edge');
  p = await b.evaluate('MK.point("chip", 1)');
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  await mouse(b, 'mousePressed', p, {button: 'middle'}); await mouse(b, 'mouseReleased', p, {button: 'middle'});
  assert.deepEqual((await state(b)).reference, {marker: 1});
  // right-click a chip: its menu removes it, undoably
  p = await b.evaluate('MK.point("chip", 2)');
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  await mouse(b, 'mousePressed', p, {button: 'right'}); await mouse(b, 'mouseReleased', p, {button: 'right'});
  assert.equal((await state(b)).menu, true);
  const labels = await b.evaluate(`[...document.querySelectorAll('.pv-menu button span')].map(e => e.textContent)`);
  assert.deepEqual(labels, ['Go to marker', 'Rename…', 'Measure from here', 'Copy as text', 'Move to cursor', 'Remove']);
  await b.click('.pv-menu button:nth-child(4)');
  await b.wait('MK.state().notice?.startsWith("Copied")');
  assert.equal(await b.evaluate('navigator.clipboard.readText()'), `lsu_ddr.vtr marker 2 “last beat seen” at 56 ns (core_clk 55, axi_clk 21)`);
  await mouse(b, 'mousePressed', p, {button: 'right'}); await mouse(b, 'mouseReleased', p, {button: 'right'});
  await b.click('.pv-menu button:last-child');
  assert.equal((await state(b)).markers.some(m => m.id === 2), false);
  await focusPanel(b); await key(b, 'z', CTRL);
  // double-click an adjacent span zooms to it
  await b.evaluate('MK.app.fit()'); await settled(b);
  p = await b.evaluate('MK.point("span", 2)');
  await dblclick(b, p); await settled(b);
  s = await state(b);
  assert.ok(s.view.a < ev.A.resp && s.view.b > ev.irq && s.view.b - s.view.a < 250000, `zoomed to 2 → 3: ${JSON.stringify(s.view)}`);
  // double-click the time header adds a marker there (on the lane, a span takes the double-click)
  const header = await b.evaluate(`MK.point("header", 150000)`);
  await dblclick(b, header);
  s = await state(b); assert.equal(s.markers.length, 4); assert.equal(s.markers.find(m => m.id === 4).time, s.cursor);
  assert.deepEqual(b.exceptions, []);
});

test('crowding: chips never overlap, clusters zoom, and both themes stay readable', {timeout: 40000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.evaluate(`(() => { MK.base(); for (let i = 0; i < 40; i++) MK.app.markers.add(150000 + i * 1300, 'm' + i); MK.app.changed(); })()`);
  let s = await state(b);
  for (let i = 1; i < s.chips.length; i++) assert.ok(s.chips[i - 1].x + s.chips[i - 1].w <= s.chips[i].x, 'chips do not overlap');
  const covered = s.chips.flatMap(c => c.ids).sort((x, y) => x - y);
  assert.deepEqual(covered, s.markers.map(m => m.id).sort((x, y) => x - y), 'every marker is in exactly one chip');
  const cluster = s.chips.find(c => c.kind === 'cluster');
  assert.ok(cluster, 'close markers merge into a cluster');
  await focusPanel(b);
  const r = await b.evaluate('(() => { const r = MK.app.canvas.getBoundingClientRect(); return {x: r.left, y: r.top}; })()');
  const g = await b.evaluate('MK.app.wv.g');
  await click(b, {x: r.x + cluster.x + cluster.w / 2, y: r.y + g.lane.y + 11});
  await settled(b);
  s = await state(b);
  assert.ok(s.chips.length > 1 && s.view.b - s.view.a < 100000, 'clicking the cluster zoomed in');
  // theme contrast of text drawn on the canvas, and painted colours matching the palette
  for (const theme of ['light', 'dark']) {
    await b.evaluate(`document.documentElement.dataset.theme = '${theme}'`);
    const c = await b.evaluate(`(() => { const P = MK.PAL['${theme}'], k = MK.contrast;
      const on = bg => [P.ink, P.text, P.panel].reduce((a, x) => k(x, bg) > k(a, bg) ? x : a);
      return {chips: P.markers.map(m => k(on(m), m)), muted: k(P.muted, P.panel), span: k(P.span, P.panel), cursor: k(P.cursor, P.panel), cursorText: k(P.cursorText, P.cursor), cluster: k(P.clusterText, P.cluster), flag: k(P.flag, P.panel)}; })()`);
    for (const [name, v] of Object.entries(c)) for (const x of [v].flat()) assert.ok(x >= 4.5, `${theme} ${name} contrast ${x.toFixed(2)}`);
    await b.evaluate(`MK.base({view: [0, 80000], cursor: 70000})`);
    const px = await b.evaluate(`(() => { const g = MK.app.wv.g, x = Math.round(MK.app.wv.x(MK.ev.A.resp)); return {px: MK.pixel(x, g.rowsTop + 13 * 22 + 11), want: MK.PAL['${theme}'].markers[1]}; })()`);
    const want = px.want.match(/\w\w/g).map(h => parseInt(h, 16));
    assert.ok(px.px.slice(0, 3).every((v, i) => Math.abs(v - want[i]) < 60), `${theme}: marker 2's line is painted in its colour (${px.px} vs ${want})`);
  }
  assert.deepEqual(b.exceptions, []);
});
