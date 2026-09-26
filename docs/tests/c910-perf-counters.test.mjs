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

test('row styles: the row switch, the ROB raster and its age order', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  const styles = ['bars', 'horizon', 'fan', 'density', 'raster', 'lqraster', 'iqh', 'state', 'istack'];
  let s = await state(b);
  assert.ok(styles.every(id => s.ids.includes(id)) && s.ids.includes('topdown'), 'all rows by default');
  await b.click('#seg-r [data-v="style"]');
  s = await state(b);
  assert.deepEqual(s.ids, ['phase', ...styles], 'row styles alone');
  await b.click('#seg-r [data-v="signal"]');
  s = await state(b);
  assert.ok(!s.ids.some(id => styles.includes(id)) && s.ids.includes('rob'), 'signals alone');
  await b.click('#seg-r [data-v="style"]');
  assert.equal(await b.evaluate('document.querySelector("#seg-a button").disabled'), true, 'age order applies to cycle-level views only');
  await b.click('[data-view="0"]');
  // In age order the valid entries sit at the bottom lanes: at the cursor, lane 0 is lit exactly when the ROB is not empty.
  await b.click('#seg-a [data-v="age"]');
  const r = await b.evaluate('PF.rowRect("raster")');
  const found = await b.evaluate(`(() => {
    const M = PF.st.M, g = PF.st.g, v = PF.st.v, left = document.getElementById("cv").getBoundingClientRect().left;
    for (let i = Math.ceil(v.b0) + 5; i < v.b1 - 5; i++) if (M.series.rob[i] >= 20) return {x: left + PFL.xOf(v, g, i + 0.5), n: M.series.rob[i]};
    return null; })()`);
  assert.ok(found, 'the stretch has a cycle with at least 20 ROB entries');
  const lane = (k) => r.y + r.h - 2 - (k + 0.5) * (r.h - 4) / 64;
  const lit = px => px[1] > 90;   // the raster's teal against the dark background
  assert.ok(lit(await b.evaluate(`PF.pixel(${found.x}, ${lane(1)})`)), 'the oldest entries are lit at the bottom');
  assert.ok(!lit(await b.evaluate(`PF.pixel(${found.x}, ${lane(62)})`)), 'the top lanes are empty with fewer than 62 entries');
  await mouse(b, 'mouseMoved', {x: found.x, y: lane(10)}, {button: 'none'});
  s = await state(b);
  assert.equal(s.values.raster, `${found.n} valid`, 'the raster reads the popcount at the pointer');
  assert.match(s.values.iqh, /^\d+ \d+ \d+ \d+ \d+$/, 'the horizon group reads each queue');
  assert.match(s.values.state, /renaming|front end|bad speculation|back end/);
  assert.deepEqual(b.exceptions, []);
});


test('row style gallery: one painted row per style, grouped by status', {timeout: 30000}, async t => {
  const b = await open(390); t.after(() => b.close());
  await b.wait('GAL.sized()');
  const cards = await b.evaluate('GAL.state()');
  const count = st => cards.filter(c => c.status === st).length;
  assert.deepEqual([count('volna'), count('proposed'), count('deferred'), count('rejected')], [4, 13, 3, 11]);
  for (const st of ['volna', 'proposed', 'deferred', 'rejected'])
    assert.equal(await b.evaluate(`document.getElementById("sty-n-${st}").textContent`), String(count(st)), `the ${st} heading counts its cards`);
  for (const c of cards) {
    assert.ok(await b.evaluate(`GAL.painted("${c.id}")`) > 0.03, `${c.id} draws its row`);
    const dts = await b.evaluate(`[...document.querySelectorAll("#sty-${c.id} dt")].map(e => e.textContent)`);
    assert.equal(dts.length, 3, `${c.id} explains itself`);
  }
  assert.deepEqual(b.exceptions, []);
});

test('row style gallery: controls, hover readouts and the age-ordered raster', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  await b.evaluate('document.getElementById("sty-line").scrollIntoView({block: "start", behavior: "instant"})');
  await b.wait('GAL.sized()');
  const at = async (id, f, fy) => b.evaluate(`GAL.pointAt("${id}", ${f}, ${fy ?? 0.5})`);
  const value = async id => (await b.evaluate('GAL.state()')).find(c => c.id === id);
  const before = await b.evaluate('GAL.painted("line")');
  await b.click('#sty-line .sty-ctl [data-v="linear"]');
  assert.equal((await value('line')).s.draw, 'linear');
  assert.notEqual(await b.evaluate('GAL.painted("line")'), before, 'linear redraws the row');
  await mouse(b, 'mouseMoved', await at('line', 0.5), {button: 'none'});
  assert.match((await value('line')).value, /^\d+ entries$/, 'hover reads the sample');
  // In age order the oldest entries sit in the bottom lanes.
  await b.evaluate('document.getElementById("sty-raster").scrollIntoView({block: "start", behavior: "instant"})');
  await b.click('#sty-raster .sty-ctl [data-v="age"]');
  const r = await b.evaluate(`(() => { const s = PFL.build(0, 1, 'count').series.rob; for (let i = 100; i < 1000; i++) if (s[i] >= 20 && s[i + 1] >= 20 && s[i - 1] >= 20) return i; return -1; })()`);
  assert.ok(r > 0, 'the stretch holds 20 entries somewhere');
  const x = (await at('raster', (r + 0.5) / 1024)).x, top = (await at('raster', 0, 0)).y, h = 72;
  const lane = k => top + h - 2 - (k + 0.5) * (h - 4) / 64;
  assert.ok((await b.evaluate(`GAL.pixel("raster", ${x}, ${lane(1)})`))[1] > 90, 'the oldest lanes are lit');
  assert.ok((await b.evaluate(`GAL.pixel("raster", ${x}, ${lane(62)})`))[1] < 90, 'the top lanes are empty');
  await mouse(b, 'mouseMoved', {x, y: lane(10)}, {button: 'none'});
  assert.match((await value('raster')).value, /valid$/);
  for (const [id, re] of [['state', /retiring|front end|bad speculation|back end/], ['pie', /%$/], ['dual', /^\d\.\d\d · \d\.\d\d$/]]) {
    await b.evaluate(`document.getElementById("sty-${id}").scrollIntoView({block: "start", behavior: "instant"})`);
    await mouse(b, 'mouseMoved', await at(id, 0.4), {button: 'none'});
    assert.match((await value(id)).value, re, `${id} reads the value under the pointer`);
  }
  // The adjustable horizon redraws for a new baseline.
  const h0 = await b.evaluate('GAL.painted("hadj")');
  await b.evaluate(`(() => { const i = document.querySelector("#sty-hadj input"); i.value = "2.5"; i.dispatchEvent(new Event("input")); })()`);
  assert.equal((await value('hadj')).s.base, 2.5);
  assert.equal(await b.evaluate('document.querySelector("#sty-hadj output").textContent'), '2.50');
  assert.notEqual(await b.evaluate('GAL.painted("hadj")'), h0);
  assert.deepEqual(b.exceptions, []);
});
