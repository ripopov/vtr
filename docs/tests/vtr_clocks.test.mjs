// node --test docs/tests/vtr_clocks.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../vtr_clocks.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const state = b => b.evaluate('CLK.state()');
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
async function open(width) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 1000, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.wait('document.fonts.status === "loaded"');   // web fonts reflow the page when they arrive
  await b.evaluate('document.getElementById("cw").scrollIntoView({block: "center", behavior: "instant"})');
  await b.wait('CLK.sized()');   // the canvas resize observer runs after the viewport override
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
  assert.ok(await b.evaluate('document.querySelectorAll("main table").length') <= 2, 'at most two tables');
  assert.equal(await b.evaluate('document.getElementById("ccv").clientWidth > 250'), true, 'the canvas is visible');
  assert.deepEqual(b.exceptions, []);
});

test('the file holds few stretches of begin, end and period', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  const core = await b.evaluate('CLK.segments("core_clk")');
  assert.deepEqual(core.map(s => s.edges), [59, 60, 50]);
  assert.deepEqual(core.map(s => s.cycle), [0, 59, 119], 'numbering continues across frequency changes');
  assert.deepEqual(await b.evaluate('CLK.record("core_clk")'),
    [{begin: 400, end: 19772, period: 334}, {begin: 20272, end: 49772, period: 500}, {begin: 50772, end: 99772, period: 1000}],
    'only begin, end and period are stored');
  assert.deepEqual(await b.evaluate('[1,2,3,4].map(k => CLK.edge("core_clk", k))'), [734, 1068, 1402, 1736], 'the 3 GHz request runs at 334 ps');
  assert.equal((await b.evaluate('CLK.segments("periph_clk")')).length, 2, 'gating splits the clock');
  const gap = await b.evaluate('CLK.at("periph_clk", 50000)');
  assert.equal(gap.stopped, true); assert.equal(gap.cycle, 8);
  assert.deepEqual(b.exceptions, []);
});

test('guided scenarios drive axis, readouts and measurement', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  await b.click('[data-guide="axis"]');
  let s = await state(b);
  assert.equal(s.axis, 'core_clk'); assert.match(s.side, /core_clk\s*cycle \d+/);
  await b.click('[data-guide="gated"]');
  s = await state(b);
  assert.match(s.side, /periph_clk\s*stopped · cycle 8/);
  await b.click('[data-guide="measure"]');
  s = await state(b);
  assert.match(s.side, /Δ time\s*40 ns/); assert.match(s.side, /bus_clk\s*20 cycles/);
  await b.click('[data-guide="rounded"]');
  s = await state(b);
  assert.match(s.side, /400\s*19772\s*334/);
  assert.deepEqual(b.exceptions, []);
});

test('clicks snap to edges, brackets step cycles, rulers select clocks', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  await b.click('[data-guide="dvfs"]');
  const e = await b.evaluate('CLK.edge("core_clk", 61)');
  const p = await b.evaluate(`CLK.point(${e} + 40, 0)`);
  await click(b, p);
  let s = await state(b);
  assert.equal(s.cursor, e, 'the click snapped to the nearest core_clk edge');
  await key(b, ']', 'BracketRight', 221);
  assert.equal((await state(b)).cursor, await b.evaluate('CLK.edge("core_clk", 62)'));
  await key(b, '[', 'BracketLeft', 219);
  await key(b, '[', 'BracketLeft', 219);
  assert.equal((await state(b)).cursor, await b.evaluate('CLK.edge("core_clk", 60)'));
  const rp = await b.evaluate('CLK.rulerPoint("bus_clk", 20500)');
  await click(b, rp);
  assert.equal((await state(b)).active, 'bus_clk');
  await key(b, ']', 'BracketRight', 221);
  const bus = await b.evaluate(`CLK.at("bus_clk", ${(await state(b)).cursor})`);
  assert.equal(bus.frac, 0, 'stepping lands on a bus_clk edge');
  await b.click('[data-axis="bus_clk"]');
  assert.equal((await state(b)).axis, 'bus_clk');
  await b.evaluate('document.getElementById("cw").focus({preventScroll: true})');   // the chip took focus
  await key(b, 'm', 'KeyM', 77);
  assert.equal((await state(b)).marker, (await state(b)).cursor);
  assert.deepEqual(b.exceptions, []);
});
