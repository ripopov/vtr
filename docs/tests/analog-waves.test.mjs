// node --test docs/tests/analog-waves.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../analog-waves.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const state = b => b.evaluate('AW.state()');
const row = (b, name) => b.evaluate(`AW.row(${JSON.stringify(name)})`);
const settled = b => b.wait('!AW.state().animating');
async function mouse(b, type, p, extra = {}) {
  await b.send('Input.dispatchMouseEvent', {type, x: p.x, y: p.y, button: 'left', clickCount: 1, ...extra});
}
async function click(b, p, extra = {}) {
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  await mouse(b, 'mousePressed', p, extra);
  await mouse(b, 'mouseReleased', p, extra);
}
async function key(b, key, code, modifiers = 0, keyCode = 0) {
  for (const type of ['rawKeyDown', 'keyUp'])
    await b.send('Input.dispatchKeyEvent', {type, key, code, modifiers, windowsVirtualKeyCode: keyCode, text: type === 'rawKeyDown' && key.length === 1 ? key : undefined});
}
async function guide(b, name) { await b.click(`[data-guide="${name}"]`); await settled(b); }
async function open(width = 1280) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 900, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.evaluate('document.getElementById("pv").scrollIntoView({block: "center", behavior: "instant"})');
  // Let fonts and the scrolled layout settle before reading coordinates.
  await b.evaluate('document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))))');
  await settled(b);
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
  assert.equal(await b.evaluate('document.querySelectorAll(".callout,.card,.badge").length'), 0, 'no callout boxes');
  assert.equal(await b.evaluate('document.querySelectorAll("table").length <= 2'), true, 'at most two tables');
  assert.equal(await b.evaluate('document.getElementById("wbody").clientWidth > 200'), true, 'waves panel is visible');
  // Both figures render; the envelope shows the glitch, decimation does not.
  const glitch = await b.evaluate(`(() => { const out = {}; for (const id of ['fig-m4', 'fig-naive']) {
    const c = document.getElementById(id), g = c.getContext('2d'), x = Math.round(42010 / 100000 * c.width);
    const d = g.getImageData(x - 1, 14, 3, 14).data; let best = 0;
    for (let k = 0; k < d.length; k += 4) best = Math.max(best, d[k + 1] - (d[k] + d[k + 2]) / 2);
    out[id] = best; } return out; })()`);
  assert.ok(glitch['fig-m4'] > 40, `envelope figure draws the glitch peak (${glitch['fig-m4']})`);
  assert.ok(glitch['fig-naive'] < 20, `decimation figure misses it (${glitch['fig-naive']})`);
  assert.deepEqual(b.exceptions, []);
});

test('guided scenarios: plot, signed, zoom out, window range, glitch', {timeout: 40000}, async t => {
  const b = await open(); t.after(() => b.close());
  let r = await row(b, 'adc_in');
  assert.equal(r.analog, 'linear', 'a real signal opens as a linear plot');
  assert.equal(r.h, 3);
  assert.equal((await row(b, 'fir_out')).analog, null);

  await guide(b, 'plot');
  r = await row(b, 'fir_out');
  assert.equal(r.analog, 'step'); assert.equal(r.h, 3); assert.equal(r.fmt, 'hex');
  assert.ok(r.cur[0] < 500 && r.cur[1] > 65000, `hex plots unsigned and wraps (${r.cur})`);
  assert.deepEqual((await state(b)).selected, ['fir_out']);

  await guide(b, 'signed');
  r = await row(b, 'fir_out');
  assert.equal(r.fmt, 'sdec');
  assert.ok(r.cur[0] < -8000 && r.cur[1] > 8000, `signed range is centred on zero (${r.cur})`);
  assert.match(r.value, /^−?\d+$/);

  await guide(b, 'zoomout');
  let s = await state(b);
  assert.deepEqual(s.view, [0, 100000]);
  r = await row(b, 'adc_in');
  assert.equal(r.drawMode, 'envelope');
  const top = await b.evaluate('AW.plotPoint("adc_in", 42010)');
  const green = await b.evaluate(`AW.greenIn(${top.x}, ${top.y - 2}, ${top.y + 3})`);
  assert.ok(green > 40, `the glitch peak is painted at the top of the envelope (${green})`);

  await guide(b, 'window');
  r = await row(b, 'fir_out');
  assert.equal(r.range, 'window');
  assert.ok(r.cur[1] - r.cur[0] < 8000, `visible-window range fits the weak signal (${r.cur})`);
  assert.deepEqual(r.cur, r.tgt, 'the range settled on its target');

  await guide(b, 'glitch');
  s = await state(b);
  assert.equal(s.cursor, 42010);
  r = await row(b, 'adc_in');
  assert.equal(r.drawMode, 'exact'); assert.equal(r.value, '1.32');
  const code = await row(b, 'adc_code');
  assert.equal(code.analog, 'step'); assert.equal(code.fmt, 'sdec');
  assert.deepEqual(b.exceptions, []);
});

test('pointer and keyboard: badge menu, A, resize, hover, zoom', {timeout: 40000}, async t => {
  const b = await open(); t.after(() => b.close());

  // The badge opens the format menu with Format and Draw; choosing step plots the row.
  await click(b, await b.evaluate('AW.badgePoint("adc_code")'));
  let s = await state(b);
  assert.equal(s.menu, true);
  assert.match(s.menuText, /Hexadecimal.*Signed decimal.*Digital.*Analog · step.*Analog · linear/);
  assert.doesNotMatch(s.menuText, /Whole trace/, 'no Range section while digital');
  await b.click('.menu [data-draw="step"]');
  let r = await row(b, 'adc_code');
  assert.equal(r.analog, 'step'); assert.equal(r.h, 3);
  assert.equal((await state(b)).menu, false);

  // The Range section appears once analog; Type limits follows the format.
  await click(b, await b.evaluate('AW.badgePoint("adc_code")'));
  s = await state(b);
  assert.match(s.menuText, /Whole trace.*Visible window.*Type limits/);
  await b.click('.menu [data-range="type"]');
  await settled(b);
  assert.deepEqual((await row(b, 'adc_code')).cur, [0, 4095]);

  // Bit rows have no Draw section.
  await click(b, await b.evaluate('AW.badgePoint("overflow")'));
  assert.doesNotMatch((await state(b)).menuText, /Analog/);
  await key(b, 'Escape', 'Escape', 0, 27);
  assert.equal((await state(b)).menu, false);

  // A toggles analog on the selection and restores the height.
  await click(b, await b.evaluate('AW.namePoint("fir_out")'));
  await key(b, 'a', 'KeyA', 0, 65);
  r = await row(b, 'fir_out'); assert.equal(r.analog, 'step'); assert.equal(r.h, 3);
  await key(b, 'a', 'KeyA', 0, 65);
  r = await row(b, 'fir_out'); assert.equal(r.analog, null); assert.equal(r.h, 1);
  // T cycles the format.
  await key(b, 't', 'KeyT', 0, 84);
  assert.equal((await row(b, 'fir_out')).fmt, 'dec');

  // Dragging a row's bottom edge snaps to the height presets.
  const edge = await b.evaluate('AW.edgePoint("agc_gain")');
  await mouse(b, 'mouseMoved', edge, {button: 'none'});
  await mouse(b, 'mousePressed', edge);
  for (let k = 1; k <= 5; k++) await mouse(b, 'mouseMoved', {x: edge.x, y: edge.y + 44 * k / 5});
  s = await state(b);
  assert.match(s.tip || '', /Height 4×/);
  await mouse(b, 'mouseReleased', {x: edge.x, y: edge.y + 44});
  assert.equal((await row(b, 'agc_gain')).h, 4);

  // Hover reads the plot: a column's min … max when dense.
  const p = await b.evaluate('AW.wavePoint("adc_in", 41000)');
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  s = await state(b);
  assert.match(s.tip || '', /adc_in .+ … .+changes? in/);

  // Right-click a name: Show as analog and Height.
  const n = await b.evaluate('AW.namePoint("fifo_level")');
  await mouse(b, 'mousePressed', n, {button: 'right'}); await mouse(b, 'mouseReleased', n, {button: 'right'});
  s = await state(b);
  assert.equal(s.menu, true); assert.match(s.menuText, /Show as analog.*Height/);
  await b.click('.menu [data-h="8"]');
  assert.equal((await row(b, 'fifo_level')).h, 8);

  // Ctrl+wheel zooms in until every sample is drawn; the wheel alone pans.
  await b.evaluate('AW.demo.animate(new (AW.demo.vp.value.constructor)(41700, 42300), 0)');
  await settled(b);
  const q = await b.evaluate('AW.wavePoint("adc_in", 42010)');
  const before = (await state(b)).view;
  await b.send('Input.dispatchMouseEvent', {type: 'mouseWheel', x: q.x, y: q.y, deltaX: 0, deltaY: -120, modifiers: 2});
  await settled(b);
  const zoomed = (await state(b)).view;
  assert.ok(zoomed[1] - zoomed[0] < (before[1] - before[0]) * 0.6, 'zoomed in');
  assert.equal((await row(b, 'adc_in')).drawMode, 'exact');
  await b.send('Input.dispatchMouseEvent', {type: 'mouseWheel', x: q.x, y: q.y, deltaX: 0, deltaY: 200});
  await settled(b);
  const panned = (await state(b)).view;
  assert.ok(Math.abs((panned[1] - panned[0]) - (zoomed[1] - zoomed[0])) < 1e-6 && panned[0] > zoomed[0], 'panned without zoom');

  // Shift+→ steps to the next change of the selected row; a click snaps to a sample.
  await click(b, await b.evaluate('AW.namePoint("adc_in")'));
  await b.evaluate('AW.demo.cursor = 42013');
  await key(b, 'ArrowRight', 'ArrowRight', 8, 39);
  assert.equal((await state(b)).cursor, 42020);
  const sample = await b.evaluate('AW.wavePoint("adc_in", 42032)');
  await click(b, sample);
  assert.equal((await state(b)).cursor, 42030);
  assert.deepEqual(b.exceptions, []);
});
