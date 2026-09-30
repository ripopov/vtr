// node --test docs/tests/stacked-areas.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browser} from './design-system-lib.mjs';

const PAGE = new URL('../stacked-areas.html', import.meta.url).pathname;
const html = await readFile(PAGE, 'utf8');
const literal = text => JSON.parse(text.match(/const DATA = (\{.*?\});\n/s)[1]);

const state = b => b.evaluate('SA.state()');
const call = (b, fn, ...args) => b.evaluate(`SA.${fn}(${args.map(a => JSON.stringify(a)).join(',')})`);
const settle = b => b.evaluate('SA.settle().then(() => true)');
async function mouse(b, type, p, extra = {}) {
  await b.send('Input.dispatchMouseEvent', {type, x: p.x, y: p.y, button: 'left', clickCount: 1, ...extra});
}
async function click(b, p, extra = {}) {
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  await mouse(b, 'mousePressed', p, extra);
  await mouse(b, 'mouseReleased', p, extra);
}
async function drag(b, from, to) {
  await mouse(b, 'mouseMoved', from, {button: 'none'});
  await mouse(b, 'mousePressed', from);
  for (let k = 1; k <= 6; k++) await mouse(b, 'mouseMoved', {x: from.x + (to.x - from.x) * k / 6, y: from.y + (to.y - from.y) * k / 6}, {buttons: 1});
  await mouse(b, 'mouseReleased', to);
}
const SHIFT = 8, CTRL = 2;
async function key(b, key, code, modifiers = 0, keyCode = 0) {
  for (const type of ['rawKeyDown', 'keyUp'])
    await b.send('Input.dispatchKeyEvent', {type, key, code, modifiers, windowsVirtualKeyCode: keyCode,
      text: type === 'rawKeyDown' && key.length === 1 && !(modifiers & CTRL) ? key : undefined});
}
async function guide(b, name) { await b.evaluate(`document.querySelector('[data-guide="${name}"]').click()`); await settle(b); }
async function open(width = 1280) {
  const b = await browser();
  await b.open(PAGE, {width, height: 1000});
  await b.wait('window.ready === true');
  await b.evaluate('document.getElementById("pv").scrollIntoView({block: "start", behavior: "instant"})');
  await settle(b);
  return b;
}
const members = s => s.rows.slice(1).filter(r => r.depth > 0).map(r => r.p);

test('embedded data matches the parent proposal', async () => {
  const S = literal(html), P = literal(await readFile(new URL('../c910-perf-counters.html', import.meta.url), 'utf8'));
  assert.equal(S.cycles, P.cycles); assert.equal(S.issued, P.totals.issued);
  assert.equal(S.period, P.period); assert.equal(S.first, P.first); assert.equal(S.window, P.window);
  for (const [k, v] of Object.entries(S.win)) assert.deepEqual(v, P.win[k], `win.${k}`);
  assert.equal(S.details.length, P.details.length);
  S.details.forEach((d, i) => {
    assert.equal(d.name, P.details[i].name); assert.equal(d.from, P.details[i].from_cycle);
    for (const [k, v] of Object.entries(d.series)) assert.equal(v, P.details[i].series[k], `${d.name}.${k}`);
  });
});

test('the page self-test passes and the page loads cleanly', async t => {
  const b = await open(); t.after(() => b.close());
  const out = await call(b, 'selfTest');
  assert.match(out, /selftest: all passed/, out);
  assert.equal(await b.evaluate('document.getElementById("selftest").textContent.includes("all passed")'), true);
  assert.deepEqual(b.exceptions, []);
});

test('stacking a group: key, sum, layer pixels, readout and undo', async t => {
  const b = await open(); t.after(() => b.close());
  let s = await state(b);
  assert.equal(s.stacked, false, 'the group opens drawn as today');
  assert.deepEqual(s.sel, [0]);
  await b.evaluate('document.getElementById("waves").focus()');
  await key(b, 'A', 'KeyA', SHIFT, 65);
  s = await state(b);
  assert.equal(s.stacked, true);
  assert.equal(s.rows[0].h, 3, 'a 1× group grows to 3×');
  assert.deepEqual(s.steps, ['Stack issue']);
  assert.equal(s.say, 'Stack issue');
  // The cursor sits on the busiest cycle of the matrix stretch.
  const r = await call(b, 'readout', s.cursor);
  assert.equal(r.total, 5);
  assert.equal(await b.evaluate('SA.state().members.length'), 8);
  assert.match(await b.evaluate('document.getElementById("waves").getAttribute("aria-label")'), /stacked area, 8 layers, sum 5/);
  // Rendered output: a layer issuing at the cursor is painted in its ladder colour.
  const p = r.parts.find(x => x.value === 1).p;
  const pt = await call(b, 'layerPoint', s.cursor, p);
  assert.deepEqual(await call(b, 'pixel', pt.x, pt.y), await call(b, 'layerColour', p));
  // Hovering the layer lists every part and the total, and names the hovered pipe.
  await mouse(b, 'mouseMoved', pt, {button: 'none'});
  await settle(b);
  s = await state(b);
  assert.ok(s.tip, 'the readout shows');
  assert.match(s.tip, /Σ 8 layers\s*5/);
  assert.equal(await b.evaluate(`document.querySelector('#tip tr.is-hot').dataset.p`), String(p));
  // Undo and redo through the journal.
  await key(b, 'z', 'KeyZ', CTRL, 90);
  s = await state(b);
  assert.equal(s.stacked, false); assert.equal(s.rows[0].h, 1); assert.equal(s.say, 'Undo Stack issue');
  await key(b, 'Z', 'KeyZ', CTRL | SHIFT, 90);
  assert.equal((await state(b)).stacked, true);
});

test('the menu stacks and toggles the peak band', async t => {
  const b = await open(); t.after(() => b.close());
  const g = await call(b, 'rowPoint', 0, 'name');
  await click(b, g, {button: 'right'});
  await settle(b);
  assert.equal((await state(b)).menu, true);
  await b.evaluate(`document.querySelector('#menu [data-act="stack"]').click()`);
  let s = await state(b);
  assert.equal(s.stacked, true); assert.equal(s.menu, false);
  await click(b, g, {button: 'right'});
  await b.evaluate(`document.querySelector('#menu [data-act="peak"]').click()`);
  s = await state(b);
  assert.equal(s.rows[0].peak, false);
  assert.deepEqual(s.steps, ['Stack issue', 'Hide peak of total']);
});

test('dragging rows reorders layers and takes them out of the group', async t => {
  const b = await open(); t.after(() => b.close());
  await guide(b, 'stack');
  // pipe3 is row 4; drop it just above the last member's lower edge: it becomes the baseline layer.
  await drag(b, await call(b, 'rowPoint', 4, 'name'), await call(b, 'dropPoint', 8));
  let s = await state(b);
  assert.deepEqual(members(s), [0, 1, 2, 4, 5, 6, 7, 3]);
  assert.match(s.say, /^Move ctrl_rf_pipe3_pipedown_vld$/);
  // Just below the group's last row, the row leaves the group.
  await drag(b, await call(b, 'rowPoint', 7, 'name'), await call(b, 'dropPoint', 8, true));
  s = await state(b);
  assert.deepEqual(members(s), [0, 1, 2, 4, 5, 6, 3]);
  assert.deepEqual(s.rows.at(-1), {type: 'signal', p: 7, depth: 0});
  const r = await call(b, 'readout', s.cursor);
  assert.equal(r.parts.length, 7, 'the stack sums its members only');
  await key(b, 'z', 'KeyZ', CTRL, 90); await key(b, 'z', 'KeyZ', CTRL, 90);
  assert.deepEqual(members(await state(b)), [0, 1, 2, 3, 4, 5, 6, 7]);
});

test('guides: whole-run means, baseline, leaving out the FPUs, folding', async t => {
  const b = await open(); t.after(() => b.close());
  await guide(b, 'read');
  let s = await state(b);
  assert.match(s.tip, /Σ 8 layers\s*5/, 'the readout is pinned at the cursor');
  await guide(b, 'run');
  s = await state(b);
  assert.deepEqual(s.view, {a: 0, b: 995 * 256});
  const g = await call(b, 'rowPoint', 0, 'waves');
  await mouse(b, 'mouseMoved', {x: g.x, y: g.y + 20}, {button: 'none'});
  await settle(b);
  assert.match((await state(b)).tip, /means over \d{3} cycles/);
  await guide(b, 'baseline');
  s = await state(b);
  assert.equal(members(s).at(-1), 3, 'loads on the baseline');
  await guide(b, 'fpu');
  s = await state(b);
  assert.deepEqual(members(s), [0, 1, 2, 4, 5, 3]);
  assert.equal(s.steps.length, 4);
  await guide(b, 'fold');
  s = await state(b);
  assert.equal(s.rows[0].open, false); assert.equal(s.stacked, true);
  assert.equal(s.steps.length, 4, 'folding makes no step');
  // Unfold with the chevron: navigation, still no step.
  await click(b, await call(b, 'rowPoint', 0, 'chevron'));
  s = await state(b);
  assert.equal(s.rows[0].open, true); assert.equal(s.steps.length, 4);
  // Shift+→ steps the cursor to the next change of the total.
  await b.evaluate('document.getElementById("waves").focus()');
  const before = s.cursor, t0 = (await call(b, 'readout', before)).total;
  await key(b, 'ArrowRight', 'ArrowRight', SHIFT, 39);
  s = await state(b);
  assert.ok(s.cursor > before);
  assert.notEqual((await call(b, 'readout', s.cursor)).total, t0);
  assert.equal((await call(b, 'readout', s.cursor - 1)).total, t0);
});

test('phone width and the light theme', async t => {
  const b = await open(360); t.after(() => b.close());
  assert.equal(await b.evaluate('document.documentElement.scrollWidth'), 360);
  await b.evaluate(`document.querySelector('#page-theme [data-mode="light"]').click()`);
  await guide(b, 'read');
  // At phone width a cycle is about a pixel, so probe a cycle away from the cursor line.
  const s = await state(b);
  let t0 = s.cursor - 12, r = await call(b, 'readout', t0);
  while (!r.parts.some(x => x.value === 1)) r = await call(b, 'readout', --t0);
  const p = r.parts.find(x => x.value === 1).p;
  const pt = await call(b, 'layerPoint', t0, p);
  assert.deepEqual(await call(b, 'pixel', pt.x, pt.y), await call(b, 'layerColour', p));
  assert.equal(await b.evaluate('document.documentElement.scrollWidth'), 360);
  assert.deepEqual(b.exceptions, []);
});
