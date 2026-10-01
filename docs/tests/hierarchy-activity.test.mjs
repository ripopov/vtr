// node --test --test-concurrency=1 docs/tests/hierarchy-activity.test.mjs
// The hierarchy activity design: the page's stretch index answers exactly,
// with the ground truth computed from every change of every C910 signal, for
// every window at least as wide as the smallest threshold Δ of the blocks it
// touches, and brackets the truth for narrower ones. The
// demo's strip moves the window by drag, keyboard and presets, and the tree
// paints meters, ranges and quiet scopes. The design-system guardrails cover
// the page's lint, contrast and 360px layout. The staged plan's pictures are
// built from the same index, with one numbered ring per numbered addition.
import assert from 'node:assert/strict';
import {join} from 'node:path';
import {test} from 'node:test';
import {browser, root} from './design-system-lib.mjs';

const page = join(root, 'docs/hierarchy-activity.html');

// Exact answers over bench/results/latest/c910_coremark.vtr, from each signal's
// full list of change times (initial values at the first time step excluded):
// active signals, a hash of every scope's distinct active-signal count in DFS
// order (h = h * 31 + count, 32-bit), scopes with none, and two named units.
const TRUTH = [
  {t0: 220000, t1: 270000, active: 24387, hash: 3717205637, quietScopes: 663, ifu: 3272, vfpu: 16},
  {t0: 250000, t1: 252000, active: 17203, hash: 2866767085, quietScopes: 795, ifu: 2256, vfpu: 16},
  {t0: 250000, t1: 251000, active: 16274, hash: 3233789294, quietScopes: 806, ifu: 2183, vfpu: 16},
  {t0: 230000, t1: 260000, active: 23897, hash: 2532224439, quietScopes: 672, ifu: 3213, vfpu: 16},
  {t0: 240000, t1: 240100, active: 12403, hash: 1851940342, quietScopes: 829, ifu: 1852, vfpu: 16},
  {t0: 265000, t1: 265016, active: 4650, hash: 742005514, quietScopes: 1009, ifu: 985, vfpu: 7},
  {t0: 253333, t1: 253335, active: 2571, hash: 740149653, quietScopes: 1040, ifu: 570, vfpu: 1},
  {t0: 220000, t1: 222000, active: 16908, hash: 559092042, quietScopes: 802, ifu: 2482, vfpu: 16},
  {t0: 262000, t1: 270000, active: 22332, hash: 835370403, quietScopes: 667, ifu: 2766, vfpu: 16},
];

async function open(t) {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page);
  await b.wait('window.ready === true', 30000);
  return b;
}

test('the index is exact from each block\'s Δ up and brackets the truth below it', {timeout: 90000}, async t => {
  const b = await open(t);
  const f = await b.evaluate('ACT.facts()');
  assert.deepEqual([f.scopes, f.signals, f.vars, f.from, f.to], [6958, 67144, 204905, 220000, 270000]);
  // The slice spans several writer blocks, each with its own power-of-two threshold.
  assert.ok(f.starts.length >= 2 && f.starts[0] === 220000, `blocks ${f.starts}`);
  assert.ok(f.deltas.every(d => d > 0 && (d & (d - 1)) === 0), `thresholds ${f.deltas}`);
  for (const w of TRUTH) {
    const r = await b.evaluate(`ACT.query(${w.t0}, ${w.t1})`);
    const label = `[${w.t0}, ${w.t1}]`;
    if (w.t1 - w.t0 + 1 >= r.dwin) {
      assert.equal(r.undecided, 0, `${label} is at least Δ = ${r.dwin} wide, so nothing is undecided`);
      assert.deepEqual([r.active, r.hash, r.quietScopes, r.ifu[0], r.vfpu[0]], [w.active, w.hash, w.quietScopes, w.ifu, w.vfpu], `${label} exact`);
    } else {
      assert.ok(r.active <= w.active && w.active <= r.active + r.undecided, `${label}: ${r.active} + ${r.undecided} brackets ${w.active}`);
      assert.ok(r.ifu[0] <= w.ifu && w.ifu <= r.ifu[1] && r.vfpu[0] <= w.vfpu && w.vfpu <= r.vfpu[1], `${label} unit ranges`);
      if (r.undecided === 0) assert.equal(r.hash, w.hash, `${label} fully decided, so exact`);
    }
  }
  // The theme proposal's figure: 16 of 5,596 vector FPU signals change in 25,000-25,100 ns.
  const theme = await b.evaluate('ACT.query(250000, 251000)');
  assert.equal(theme.vfpu[2], 5596);
  // One classification of all 67,144 signals plus the scope walk stays within a frame, with margin for slow runners.
  const ms = await b.evaluate(`(() => { const t = []; for (let i = 0; i < 9; i++) t.push(ACT.query(230000 + i * 1000, 240000 + i * 1000).ms); return t.sort((a, b) => a - b)[4]; })()`);
  assert.ok(ms < 50, `median query ${ms.toFixed(1)} ms`);
  assert.deepEqual(b.exceptions, []);
});

test('the demo moves the window and paints meters, ranges and quiet scopes', {timeout: 90000}, async t => {
  const b = await open(t);
  assert.deepEqual(await b.evaluate('[ACT.state().t0, ACT.state().t1]'), [250000, 252000]);
  assert.match(await b.evaluate(`document.getElementById('readout').textContent`), /17,203 of 67,144 signals change · exact/);
  // The default path is open down to the core's units.
  const vfpu = await b.evaluate(`ACT.row('x_ct_vfpu_top')`);
  assert.equal(vfpu.count, '16 / 5,596');
  assert.equal(vfpu.lo, '2px', 'a small share still shows');
  assert.equal((await b.evaluate(`ACT.row('x_ct_ifu_top')`)).count, '2,256 / 6,779');

  // A window narrower than Δ shows a range and says why.
  await b.click('#presets [data-w="265000,265016"]');
  await b.wait('ACT.state().t1 === 265016');
  await b.wait(`/undecided/.test(document.getElementById('readout').textContent)`);
  const narrow = await b.evaluate(`ACT.row('x_ct_ifu_top')`);
  assert.match(narrow.count, /^\d[\d,]*–\d[\d,]* \/ 6,779$/, narrow.count);
  assert.notEqual(narrow.lo, narrow.hi, 'solid and hatched parts differ');

  // Keyboard: pan and zoom on the focused strip.
  await b.click('#presets [data-w="250000,252000"]');
  await b.wait('ACT.state().t0 === 250000 && ACT.state().t1 === 252000');
  await b.evaluate(`document.getElementById('strip').focus()`);
  for (const [key, code] of [['ArrowRight', 39]]) for (const type of ['rawKeyDown', 'keyUp']) await b.send('Input.dispatchKeyEvent', {type, key, code: key, windowsVirtualKeyCode: code});
  await b.wait('ACT.state().t0 === 250200 && ACT.state().t1 === 252200');
  await b.send('Input.dispatchKeyEvent', {type: 'keyDown', key: '-', text: '-'}); await b.send('Input.dispatchKeyEvent', {type: 'keyUp', key: '-'});
  await b.wait('ACT.state().t1 - ACT.state().t0 === 2500');

  // Drag the window along the strip.
  await b.evaluate(`document.getElementById('strip').scrollIntoView({block: 'center'})`);
  const r = await b.evaluate(`(() => { const r = document.getElementById('strip').getBoundingClientRect(); return {x: r.left, y: r.top, w: r.width, h: r.height}; })()`);
  const before = await b.evaluate('ACT.state()');
  const x0 = r.x + ((before.t0 + before.t1) / 2 - 220000) / 50000 * r.w, y = r.y + r.h / 2;
  await b.send('Input.dispatchMouseEvent', {type: 'mousePressed', x: x0, y, button: 'left', buttons: 1, clickCount: 1});
  for (let k = 1; k <= 4; k++) await b.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: x0 - k * r.w / 20, y, button: 'left', buttons: 1});
  await b.send('Input.dispatchMouseEvent', {type: 'mouseReleased', x: x0 - r.w / 5, y, button: 'left', buttons: 0, clickCount: 1});
  await b.wait(`ACT.state().t0 < ${before.t0 - 8000}`);
  const after = await b.evaluate('ACT.state()');
  assert.equal(after.t1 - after.t0, before.t1 - before.t0, 'dragging keeps the width');

  // The whole slice lights every unit that moves; collapsing a scope hides its children.
  await b.click('#presets [data-w="220000,270000"]');
  await b.wait(`/24,387 of 67,144 signals change · exact/.test(document.getElementById('readout').textContent)`);
  const rows = (await b.evaluate('ACT.state()')).rows;
  await b.click(`button.a-row[aria-expanded="true"]:last-of-type`);
  await b.wait(`ACT.state().rows < ${rows}`);
  assert.deepEqual(b.exceptions, []);
});

test('the plan pictures ring every numbered addition, from the same index', {timeout: 90000}, async t => {
  const b = await open(t);
  for (const width of [1280, 360]) {
    await b.open(page, {width});
    await b.wait('window.ready === true', 30000);
    const plan = await b.evaluate('ACT.plan()');
    // Each pictured stage has one ring per item of its numbered list, all inside the picture.
    for (const shot of plan.shots) {
      const stage = shot.id.replace('shot-', 'stage-');
      const items = await b.evaluate(`document.querySelectorAll('#${stage} .a-adds > li').length`);
      assert.deepEqual(shot.marks, Array.from({length: items}, (_, k) => String(k + 1)), `${stage} marks`);
      assert.deepEqual(shot.rings.map(r => r.n).sort(), shot.marks, `${stage} rings`);
      assert.ok(shot.rings.every(r => r.inside), `${stage} rings stay inside at ${width}px`);
    }
    assert.deepEqual(plan.shots.map(s => s.id), ['shot-4', 'shot-5', 'shot-6']);
    // The command's answer needs no read, the wide meter view is exact, and the narrow one shows a range.
    assert.ok(plan.exact3 && plan.exact5 && plan.hatched5 && plan.quiet >= 0, JSON.stringify(plan));
    assert.equal(await b.evaluate('document.documentElement.scrollWidth'), width, 'no horizontal page scroll');
  }
  // The terminal answer is the exact one: 25,000-25,500 ns is wider than every Δ it touches.
  const term = await b.evaluate(`document.getElementById('term-3').textContent`);
  assert.match(term, /25,000\.0–25,500\.0 ns · 17,977 of 67,144 signals change · exact/);
  assert.match(term, /x_ct_vfpu_top +16 \/  5,596/);
  // Seven stages, each linked from the summary table; scope sizes live in their own design.
  assert.equal(await b.evaluate(`document.querySelectorAll('#plan .a-stage').length`), 7);
  assert.equal(await b.evaluate(`[...document.querySelectorAll('#plan tbody a')].filter(a => document.querySelector(a.getAttribute('href'))).length`), 7);
  assert.equal(await b.evaluate(`document.querySelector('#stage-6 .v-chip').textContent`), 'Landed');
  assert.ok(await b.evaluate(`!!document.querySelector('#stage-6 a[href="BENCHMARK_RESULTS.md#activity-sidecar-decoding-for-remote-traces"]')`));
  assert.equal(await b.evaluate(`document.querySelector('#stage-7 .v-chip').textContent`), 'Deferred');
  assert.ok(await b.evaluate(`!!document.querySelector('#stage-7 a[href="BENCHMARK_RESULTS.md#activity-reader-memory-paging-gate"]')`));
  assert.ok(await b.evaluate(`!!document.querySelector('#plan a[href="hierarchy-scope-sizes.html"]')`));
  assert.deepEqual(b.exceptions, []);
});
