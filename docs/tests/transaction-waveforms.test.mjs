// node --test docs/tests/transaction-waveforms.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../transaction-waveforms.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const state = b => b.evaluate('TXW.state()');
const settled = b => b.wait('!TXW.state().animating');
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
    await b.send('Input.dispatchKeyEvent', {type, key, code, modifiers, windowsVirtualKeyCode: keyCode});
}
async function guide(b, name) { await b.click(`[data-guide="${name}"]`); await settled(b); }

for (const width of [1280, 390]) test(`page layout, self-test and accessibility at ${width}px`, {timeout: 30000}, async t => {
  const b = await browserTest(routes); t.after(() => b.close());
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 900, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /selftest: all passed/, selftest);
  assert.doesNotMatch(selftest, /FAIL/);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links resolve');
  const unnamed = await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`);
  assert.equal(unnamed, 0, 'buttons have names');
  assert.equal(await b.evaluate('document.querySelectorAll(".callout,.card,.badge").length'), 0, 'no callout boxes');
  assert.equal(await b.evaluate('document.getElementById("wbody").clientWidth > 200'), true, 'waves panel is visible');
  assert.deepEqual(b.exceptions, []);
});

test('guided scenarios drive selection, folding, density and edges', {timeout: 40000}, async t => {
  const b = await browserTest(routes); t.after(() => b.close());
  await b.send('Emulation.setDeviceMetricsOverride', {width: 1280, height: 900, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  let s = await state(b);
  assert.equal(s.rows.indexOf('lane:3'), 10, 'lane starts below the R channel at 3× height');
  assert.equal(s.mode, 'bars');

  await guide(b, 'error');
  s = await state(b);
  assert.equal(s.selTx, 'T2'); assert.equal(s.cursor, 38);
  assert.match(s.txPanel, /SLVERR/); assert.match(s.txPanel, /0x00003000/);
  const p2 = await b.evaluate('TXW.barPoint("T2")');
  const [r, g] = await b.evaluate(`TXW.pixel(${p2.x}, ${p2.y})`);
  assert.ok(r > 180 && g < 150, `the error bar is painted red (${r},${g})`);

  await guide(b, 'move');
  s = await state(b);
  assert.deepEqual(s.rows.slice(3, 6), ['araddr', 'arid', 'lane:3'], 'lane sits under the AR channel');

  await guide(b, 'fold');
  s = await state(b);
  assert.ok(s.rows.includes('lane:1')); assert.equal(s.mode, 'bars · folded');

  await guide(b, 'zoomout');
  assert.equal((await state(b)).mode, 'density');

  await b.click('[data-guide="step"]');
  await b.wait('TXW.state().cursor === 26');
  assert.equal((await state(b)).selTx, null);
  assert.deepEqual(b.exceptions, []);
});

test('pointer and keyboard follow the wave panel rules', {timeout: 40000}, async t => {
  const b = await browserTest(routes); t.after(() => b.close());
  await b.send('Emulation.setDeviceMetricsOverride', {width: 1280, height: 900, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.evaluate('document.getElementById("pv").scrollIntoView({block: "center", behavior: "instant"})');

  // A bar click selects that transaction and moves the cursor onto it.
  const p0 = await b.evaluate('TXW.barPoint("T0")');
  await click(b, p0);
  let s = await state(b);
  assert.equal(s.selTx, 'T0'); assert.ok(s.cursor >= 4 && s.cursor <= 20, `cursor ${s.cursor}`);
  assert.match(s.txPanel, /0x00001000/);

  // Shift+→ on the lane steps between transaction boundaries.
  await b.evaluate('TXW.demo.cursor = 8');
  await key(b, 'ArrowRight', 'ArrowRight', 8, 39);
  assert.equal((await state(b)).cursor, 18);

  // Escape clears the transaction selection first; the panel keeps its content.
  await key(b, 'Escape', 'Escape', 0, 27);
  s = await state(b);
  assert.equal(s.selTx, null); assert.match(s.txPanel, /not selected/);

  // Right-click the lane name: Height 1× folds the lane.
  const n = await b.evaluate('TXW.namePoint("lane")');
  await mouse(b, 'mousePressed', n, {button: 'right'}); await mouse(b, 'mouseReleased', n, {button: 'right'});
  assert.equal((await state(b)).menu, true);
  await b.click('.menu [data-h="1"]');
  s = await state(b);
  assert.equal(s.menu, false); assert.ok(s.rows.includes('lane:1'));

  // Dragging a name cell reorders rows: move the lane to the top.
  const lane = await b.evaluate('TXW.namePoint("lane")'), clk = await b.evaluate('TXW.namePoint("clk")');
  await mouse(b, 'mouseMoved', lane, {button: 'none'});
  await mouse(b, 'mousePressed', lane);
  for (let k = 1; k <= 6; k++) await mouse(b, 'mouseMoved', {x: lane.x, y: lane.y + (clk.y - 6 - lane.y) * k / 6});
  await mouse(b, 'mouseReleased', {x: lane.x, y: clk.y - 6});
  assert.equal((await state(b)).rows[0], 'lane:1');

  // Delete removes the selected lane; the hierarchy adds it back.
  await key(b, 'Delete', 'Delete', 0, 46);
  assert.equal((await state(b)).rows.some(r => r.startsWith('lane')), false);
  await b.click('[data-side="add"]');
  assert.equal((await state(b)).rows.some(r => r.startsWith('lane')), true);

  // Ctrl+wheel zooms; the wheel alone pans.
  const before = (await state(b)).view;
  await b.send('Input.dispatchMouseEvent', {type: 'mouseWheel', x: p0.x, y: p0.y, deltaX: 0, deltaY: 240, modifiers: 2});
  await settled(b);
  const zoomed = (await state(b)).view;
  assert.ok(zoomed[1] - zoomed[0] > (before[1] - before[0]) * 2, 'zoomed out');
  await b.send('Input.dispatchMouseEvent', {type: 'mouseWheel', x: p0.x, y: p0.y, deltaX: 0, deltaY: 200});
  await settled(b);
  const panned = (await state(b)).view;
  assert.ok(Math.abs((panned[1] - panned[0]) - (zoomed[1] - zoomed[0])) < 1e-6 && panned[0] > zoomed[0], 'panned without zoom');
  assert.deepEqual(b.exceptions, []);
});
