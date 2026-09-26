// node --test docs/tests/c910-perf-counters.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../c910-perf-counters.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const state = b => b.evaluate('PF.state()');
async function mouse(b, type, p, extra = {}) {
  await b.send('Input.dispatchMouseEvent', {type, x: p.x, y: p.y, button: 'left', clickCount: 1, ...extra});
}
async function click(b, p) {
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  await mouse(b, 'mousePressed', p);
  await mouse(b, 'mouseReleased', p);
}
async function key(b, key, code, keyCode) {
  for (const type of ['rawKeyDown', 'keyUp'])
    await b.send('Input.dispatchKeyEvent', {type, key, code, windowsVirtualKeyCode: keyCode});
}
const hexRgb = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));
const near = (a, b, tol = 14) => a.every((v, i) => Math.abs(v - b[i]) <= tol);
async function open(width) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 1000, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.wait('document.fonts.status === "loaded"');
  await b.evaluate('document.getElementById("pv").scrollIntoView({block: "start", behavior: "instant"})');
  await b.wait('PF.sized()');
  return b;
}

for (const width of [1280, 390]) test(`page layout, self-test and accessibility at ${width}px`, {timeout: 30000}, async t => {
  const b = await open(width); t.after(() => b.close());
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /selftest: all passed/, selftest);
  assert.doesNotMatch(selftest, /FAIL/);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links resolve');
  const unnamed = await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`);
  assert.equal(unnamed, 0, 'buttons have names');
  assert.equal(await b.evaluate('document.getElementById("cv").clientWidth > 300'), true, 'the canvas is visible');
  assert.equal(await b.evaluate('document.getElementById("n-inst").textContent'), '330,196', 'prose counts come from the data');
  assert.equal(await b.evaluate('document.getElementById("n-little").textContent'), '42.3');
  assert.equal(await b.evaluate('document.querySelectorAll("#phase-table tbody tr").length'), 6, 'one table row per phase');
  assert.equal(await b.evaluate('document.querySelectorAll("#lat-svg rect").length > 20'), true, 'the latency histogram is drawn');
  assert.deepEqual(b.exceptions, []);
});

test('views, windows and encodings rebuild the rows from the data', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  let s = await state(b);
  assert.equal(s.view, 'run'); assert.equal(s.n, 995); assert.equal(s.bin, 256);
  await b.click('#seg-w [data-w="4096"]');
  s = await state(b);
  assert.equal(s.n, 62); assert.equal(s.bin, 4096);
  await b.click('#seg-e [data-e="total"]');
  s = await state(b);
  assert.equal(s.names[1], 'retire.total');
  await click(b, await b.evaluate('PF.pointAt("retire", 0.9995)'));
  s = await state(b);
  assert.equal(s.cursor, 61, 'a click pins the cursor on the last window');
  assert.equal(Number(s.values.retire.replace(/,/g, '')), await b.evaluate('PFL.sum(PFL.D.win.retired.slice(0, 62 * 16))'), 'the running total at the end is the window sum');
  await b.click('#seg-e [data-e="rate"]');
  await b.click('[data-view="1"]');
  s = await state(b);
  assert.equal(s.view, 1); assert.equal(s.n, 1024); assert.equal(s.bin, 1);
  assert.equal(await b.evaluate('document.querySelector("#seg-w button").disabled'), true, 'the window size applies to the whole run only');
  assert.match(s.side, /Phase: matrix/);
  await b.click('#seg-e [data-e="count"]');
  s = await state(b);
  assert.equal(s.names[1], 'retire.count');
  await click(b, await b.evaluate('PF.pointAt("retire", 0.5)'));
  s = await state(b);
  const expected = await b.evaluate(`PFL.details[1].s.retired[${s.cursor}]`);
  assert.equal(s.values.retire, String(expected), 'the count at the cursor is the recorded per-cycle value');
  assert.match(s.values.topdown, /^[0-4]\/4 used$/);
  assert.deepEqual(b.exceptions, []);
});

test('pointer and keyboard drive the cursor and zoom; the stack is painted', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  const p = await b.evaluate('PF.pointAt("topdown", 0.3, 0.93)');
  const shares = await b.evaluate('(() => { const M = PF.st.M, i = Math.floor(PFL.binAt(PF.st.v, PF.st.g, ' + p.x + ' - document.getElementById("cv").getBoundingClientRect().left)); return M.series.td.retiring[i]; })()');
  assert.ok(shares > 0.15, 'the sampled window retires enough to cover the probed pixel');
  assert.ok(near(await b.evaluate(`PF.pixel(${p.x}, ${p.y})`), hexRgb(await b.evaluate('PF.color("retiring")'))), 'the retiring share sits at the bottom of the stack');
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  let s = await state(b);
  assert.notEqual(s.hover, null, 'hover reads the rows');
  assert.match(s.values.rob, /↑\d+/, 'the ROB row shows the window maximum');
  const before = await state(b);
  await b.send('Input.dispatchMouseEvent', {type: 'mouseWheel', x: p.x, y: p.y, deltaX: 0, deltaY: -300, modifiers: 2});
  const after = await state(b);
  assert.ok(after.b1 - after.b0 < before.b1 - before.b0, 'Ctrl+wheel zooms in');
  const lw = await b.evaluate('document.getElementById("cv").getBoundingClientRect().left + PF.st.g.LW');
  const pw = await b.evaluate('PF.st.g.PW');
  const at = v => v.b0 + (p.x - lw) / pw * (v.b1 - v.b0);
  assert.ok(Math.abs(at(after) - at(before)) < 0.05, 'the zoom keeps the window under the pointer');
  await b.evaluate('document.getElementById("pv").focus()');
  await key(b, 'f', 'KeyF', 70);
  s = await state(b);
  assert.equal(s.b0, 0); assert.equal(s.b1, 995);
  await click(b, await b.evaluate('PF.pointAt("rob", 0.5)'));
  const c = (await state(b)).cursor;
  await key(b, 'ArrowRight', 'ArrowRight', 39);
  assert.equal((await state(b)).cursor, c + 1);
  await key(b, 'Escape', 'Escape', 27);
  assert.equal((await state(b)).cursor, null);
  // Double-click a marked stretch on the phase row opens its cycle-level view.
  const x = await b.evaluate(`(() => { const d = PFL.details[2], g = PF.st.g, v = PF.st.v; return document.getElementById("cv").getBoundingClientRect().left + PFL.xOf(v, g, (d.from_cycle + 512) / 256); })()`);
  const y = (await b.evaluate('PF.rowRect("phase")')).y + 12;
  await b.send('Input.dispatchMouseEvent', {type: 'mousePressed', x, y, button: 'left', clickCount: 2});
  await b.send('Input.dispatchMouseEvent', {type: 'mouseReleased', x, y, button: 'left', clickCount: 2});
  assert.equal((await state(b)).view, 2, 'double-click opens the state stretch');
  assert.deepEqual(b.exceptions, []);
});

test('the catalogue filter hides rows of other kinds and their empty groups', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  await b.click('#cat-filter [data-k="total"]');
  const shown = await b.evaluate(`[...document.querySelectorAll('#catalogue tbody tr:not([hidden]) td:first-child')].map(td => td.textContent)`);
  assert.ok(shown.length >= 3 && shown.includes('retire.total') && shown.includes('cycles'), shown.join());
  const groups = await b.evaluate(`[...document.querySelectorAll('#catalogue tbody tr:not([hidden]) th')].length`);
  assert.ok(groups >= 2 && groups < 6, 'only groups with a matching row stay');
  await b.click('#cat-filter [data-k="all"]');
  assert.equal(await b.evaluate(`document.querySelectorAll('#catalogue tbody tr[hidden]').length`), 0);
  assert.deepEqual(b.exceptions, []);
});
