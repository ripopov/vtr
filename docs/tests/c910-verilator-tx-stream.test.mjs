// node --test docs/tests/c910-verilator-tx-stream.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../c910-verilator-tx-stream.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const state = b => b.evaluate('C9.state()');
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
  await b.wait('document.fonts.status === "loaded"');   // web fonts reflow the page when they arrive
  await b.evaluate('document.getElementById("pv").scrollIntoView({block: "center", behavior: "instant"})');
  await b.wait('C9.sized()');    // the canvas resize observer runs after the viewport override
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
  assert.equal(await b.evaluate('document.querySelectorAll("table").length'), 2, 'at most two tables in the prose');
  assert.equal(await b.evaluate('document.getElementById("cv").clientWidth > 300'), true, 'the pipeline canvas is visible');
  assert.equal(await b.evaluate('document.getElementById("n-ab").textContent'), '111', 'prose counts come from the data');
  assert.deepEqual(b.exceptions, []);
});

test('guided scenarios select the recorded instructions', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  await b.click('[data-guide="fold"]');
  let s = await state(b);
  assert.equal(s.sel, 412); assert.match(s.detail, /ROB entry shared with/); assert.match(s.detail, /fold3/);
  await b.click('[data-guide="load"]');
  s = await state(b);
  assert.match(s.selText, /^lh /); assert.match(s.detail, /AG.*DC.*DA.*WB/s);
  const p = await b.evaluate('C9.cellPoint(415, "DC")');
  assert.ok(near(await b.evaluate(`C9.pixel(${p.x}, ${p.y})`), hexRgb(await b.evaluate('C9.color("DC")'))), 'the DC cell is painted');
  await b.click('[data-guide="flush"]');
  s = await state(b);
  assert.equal(s.sel, 445); assert.equal(s.selStatus, 'ok'); assert.match(s.selText, /^beq /);
  await b.click('[data-guide="wrong"]');
  s = await state(b);
  assert.equal(s.selStatus, 'aborted'); assert.match(s.detail, /aborted \(flushed\)/);
  await b.click('[data-guide="fit"]');
  s = await state(b);
  assert.equal(s.sel, null); assert.equal(s.cursor, null);
  assert.deepEqual(b.exceptions, []);
});

test('pointer and keyboard drive selection, cursor and zoom', {timeout: 30000}, async t => {
  const b = await open(1280); t.after(() => b.close());
  await b.click('[data-guide="fit"]');
  await b.click('[data-guide="load"]');
  const p = await b.evaluate('C9.cellPoint(415, "AG")');
  await click(b, p);
  let s = await state(b);
  assert.equal(s.sel, 415, 'a click on a cell selects its instruction');
  assert.equal(s.cursor, (await b.evaluate('C9L.bySeq.get(415).stages.find(x => x.name === "AG").b')), 'and puts the cursor on its cycle');
  await key(b, 'ArrowDown', 'ArrowDown', 40);
  assert.equal((await state(b)).sel, 416);
  await key(b, 'ArrowRight', 'ArrowRight', 39);
  assert.equal((await state(b)).cursor, s.cursor + 1);
  const before = await state(b);
  await b.send('Input.dispatchMouseEvent', {type: 'mouseWheel', x: p.x, y: p.y, deltaX: 0, deltaY: -240, modifiers: 2});
  const after = await state(b);
  assert.ok(after.pxc > before.pxc, 'Ctrl+wheel zooms in');
  const at = (v, x0) => v.x0 + (p.x - x0) / v.pxc;
  const lw = await b.evaluate('document.getElementById("cv").getBoundingClientRect().left + demo.st.g.LW');
  assert.ok(Math.abs(at(after, lw) - at(before, lw)) < 0.05, 'the zoom keeps the cycle under the pointer');
  await key(b, 'Escape', 'Escape', 27);
  assert.equal((await state(b)).sel, null);
  assert.deepEqual(b.exceptions, []);
});
