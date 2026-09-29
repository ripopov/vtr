// node --test --test-concurrency=1 docs/tests/multiple-traces.test.mjs
// The multiple-traces proposal: the embedded recordings are consistent (the
// DRAM controller's FST dump matches the SoC run's refills), the page's compare
// engine agrees with an independent brute-force reading of the traces, the
// quoted numbers are the computed ones, the demo's scenarios, keys, links,
// suggestion, sidebar, placement figure and paired tables work, and each plan
// stage's picture shows only its stage's features. The design-system guardrails
// cover the page's lint, contrast and 360px layout.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {join} from 'node:path';
import {test} from 'node:test';
import {browser, root} from './design-system-lib.mjs';

const page = join(root, 'docs/multiple-traces.html');
const html = await readFile(page, 'utf8');
const RUNS = JSON.parse(html.split('\n').find(l => l.startsWith('const RUNS = ')).slice('const RUNS = '.length).replace(/;\s*$/, ''));

/** Value of a change list at time t (the last change at or before t). */
const valueAt = (ch, t) => { let v; for (const [ct, cv] of ch) { if (ct <= t) v = cv; else break; } return v; };
/** Brute force: sample both runs every half nanosecond, the other shifted by `offset`; the first differing sample per
 *  signal, or null. Exact rules; only where both runs recorded. */
function firstDifferences(other, offset) {
  const A = RUNS.base, B = RUNS[other], end = Math.min(A.range[1], B.range[1] - offset), out = {};
  for (const path of Object.keys(A.signals)) {
    out[path] = null;
    for (let t2 = 0; t2 < 2 * end; t2++) {
      const t = t2 / 2;
      if (valueAt(A.signals[path], t) !== valueAt(B.signals[path], t + offset)) { out[path] = t; break; }
    }
  }
  return out;
}
const count = d => Object.values(d).filter(t => t !== null).length;
const earliest = d => Math.min(...Object.values(d).filter(t => t !== null));
async function open(t, options) {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page, options);
  await b.wait('window.ready === true');
  return b;
}

test('the recordings are the landing generator, and the DRAM dump matches the SoC run', () => {
  assert.deepEqual(Object.keys(RUNS), ['base', 'dram20', 'reset12', 'dram']);
  assert.deepEqual([RUNS.base.range, RUNS.dram20.range, RUNS.reset12.range, RUNS.dram.range], [[0, 1685], [0, 1817], [0, 1701], [0, 1685000]]);
  assert.deepEqual([RUNS.base.format, RUNS.base.unit, RUNS.dram.format, RUNS.dram.unit], ['VTR', 'ns', 'FST', 'ps']);
  for (const id of ['base', 'dram20', 'reset12']) {
    const r = RUNS[id];
    assert.equal(r.insns.length, 420, 'every run fetches 420 instructions');
    assert.equal(Object.keys(r.signals).length, 16, 'sixteen signals besides the clock');
  }
  for (const r of Object.values(RUNS)) for (const ch of Object.values(r.signals)) for (let i = 1; i < ch.length; i++) assert.ok(ch[i][0] > ch[i - 1][0], 'changes in time order');
  // Same program: the fetched pc sequences agree, so every instruction pairs.
  assert.deepEqual(RUNS.dram20.insns.map(i => i[2]), RUNS.base.insns.map(i => i[2]));
  assert.deepEqual(RUNS.reset12.insns.map(i => i[2]), RUNS.base.insns.map(i => i[2]));
  // dram20's reads served by DRAM take 6 cycles (12 ns) longer; L2 hits are unchanged.
  const reads = r => r.bus.filter(x => x[0] === 'read').map(x => [x[3], x[4], x[2] - x[1]]);
  const a = reads(RUNS.base), b = reads(RUNS.dram20);
  assert.equal(a.length, 30);
  a.forEach(([addr, level, lat], i) => { assert.equal(b[i][0], addr); assert.equal(b[i][2] - lat, level === 'DRAM' ? 12 : 0); });
  // reset12 is the base run delayed by 8 cycles in the core.
  assert.deepEqual(RUNS.reset12.insns.map(i => i[5][1] - 16), RUNS.base.insns.map(i => i[5][1]));
  // Every DRAM burst of the SoC run is an ACT of the controller at the same instant, for the same address.
  const bursts = RUNS.base.bus.filter(x => x[0] === 'burst');
  const acts = RUNS.dram.signals['lpddr_tb.ctrl.state'].filter(c => c[1] === 'ACT').map(c => c[0]);
  assert.deepEqual(acts, bursts.map(x => x[1] * 1000));
  for (const x of bursts) assert.equal(valueAt(RUNS.dram.signals['lpddr_tb.ctrl.addr'], x[1] * 1000), x[3].toString(16).padStart(8, '0'));
});

test('the compare engine matches a brute-force reading of the traces', {timeout: 120000}, async t => {
  const b = await open(t);
  const results = await b.evaluate('MT.alignResults()');
  const pick = (pair, mode) => results.find(r => r.pair === pair && r.mode === mode);
  const bTime = firstDifferences('dram20', 0), cTime = firstDifferences('reset12', 0), cOffset = firstDifferences('reset12', 16);
  assert.equal(pick('dram20', 'time').n, count(bTime));
  assert.equal(pick('dram20', 'time').first, earliest(bTime));
  assert.equal(earliest(bTime), 177, 'the L2 response differs first');
  assert.equal(bTime['soc.l2.resp_valid'], 177);
  assert.equal(bTime['soc.cpu0.stall'], 179, 'the core stalls 2 ns later');
  assert.equal(pick('reset12', 'time').n, count(cTime));
  assert.equal(pick('reset12', 'offset').name, 'Offset +16 ns', 'the detected offset');
  assert.equal(pick('reset12', 'offset').n, count(cOffset));
  assert.deepEqual(Object.keys(cOffset).filter(p => cOffset[p] !== null).sort(), ['soc.perf.temperature_c', 'soc.reset_n']);
  assert.equal(pick('reset12', 'offset').tolerant, 0, 'reset12 matches under the tolerant rules');
  const state = mode => `{ref: 'base', traces: ['base', 'dram20'], focus: 'dram20', mode: 'compare', align: {dram20: '${mode}'}, rules: 'exact'}`;
  const engine = await b.evaluate(`Object.fromEntries(MT.withState(${state('time')}, () => MT.divergence('dram20')).map(r => [r.path, r.d.first]))`);
  for (const [path, first] of Object.entries(bTime)) {
    if (first === null) assert.equal(engine[path], null, path);
    else assert.ok(Math.abs(engine[path] - first) <= 0.5, `${path}: ${engine[path]} vs ${first}`);
  }
  // By instruction, the core's program-order signals match exactly.
  const insn = await b.evaluate(`Object.fromEntries(MT.withState(${state('insn')}, () => MT.divergence('dram20')).map(r => [r.path, r.d.first]))`);
  for (const p of ['soc.cpu0.phase', 'soc.cpu0.retired', 'soc.cpu0.flush', 'soc.cpu0.irq']) assert.equal(insn[p], null, p);
  assert.equal(insn['soc.l2.resp_valid'], 177);
  // The retire slip: eleven steps of 12 ns, 132 ns in all; the extra cycles sit where the refills are.
  const slips = await b.evaluate(`MT.withState(${state('insn')}, () => MT.slips())`);
  const steps = slips.map((s, i) => i ? s[1] - slips[i - 1][1] : s[1]).filter(d => d);
  assert.deepEqual(steps, Array(11).fill(12));
  assert.equal(Math.max(...RUNS.base.insns.map((a, i) => RUNS.dram20.insns[i][5].at(-1) - a[5].at(-1))), 132);
  const stages = await b.evaluate(`MT.withState(${state('insn')}, () => MT.stageDelta())`);
  assert.ok(stages.M >= 66, `M gains at least 11 × 6 cycles: ${JSON.stringify(stages)}`);
  assert.deepEqual(b.exceptions, []);
});

test('the page quotes the computed numbers', {timeout: 60000}, async t => {
  const b = await open(t);
  const text = id => b.evaluate(`document.getElementById(${JSON.stringify(id)}).textContent`);
  assert.equal(await text('stat-link'), '4 ns');
  assert.equal(await text('stat-slip'), '11 × 12 ns');
  assert.equal(await text('stat-first'), '177 ns');
  assert.equal(await text('al-offset'), '16 ns');
  assert.equal(await text('al-share'), '59%');
  assert.equal(await b.evaluate(`document.querySelectorAll('#al-table tbody tr').length`), 6);
  assert.match(html, /eleven steps of 12 ns, one for each DRAM refill, 132 ns in all/);
  assert.match(html, /At 177 ns the L2 response/);
  assert.match(html, /the controller activates it 4 ns later/);
  // The paired tables: 30 reads with the 11 DRAM refills 12 ns slower; ten log messages, seven with other text (six
  // refill times and the self-refresh time).
  assert.equal(await b.evaluate(`document.querySelectorAll('#vw-table tbody tr').length`), 30);
  assert.equal(await b.evaluate(`document.querySelectorAll('#vw-table tbody tr.is-diff').length`), 11);
  await b.click('#vw-tabs [data-tab="logs"]');
  assert.equal(await b.evaluate(`document.querySelectorAll('#vw-table tbody tr').length`), 10);
  assert.match(await text('vw-caption'), /^10 messages paired by their text with the numbers taken out\. 7 say something different in B, and 9 arrive later\.$/);
  // Every stage of the plan has its picture and caption.
  for (let n = 1; n <= 8; n++) {
    assert.ok(await b.evaluate(`document.getElementById('stage-${n}-canvas').height > 200`), `stage ${n} picture`);
    assert.ok((await text(`stage-${n}-caption`)).length > 40, `stage ${n} caption`);
  }
  assert.deepEqual(b.exceptions, []);
});

test('the demo drives every scenario, key and control', {timeout: 120000}, async t => {
  const b = await open(t, {width: 1440, height: 1000});
  const state = () => b.evaluate(`(() => { const S = MT.S; return {mode: S.mode, traces: S.traces.slice(), focus: S.focus, align: {...S.align}, rules: S.rules,
    cursor: S.cursor, view: S.view.slice(), selected: S.selected, blink: S.blink, link: S.link, side: S.side, added: S.added.map(a => a.path)}; })()`);
  const text = id => b.evaluate(`document.getElementById(${JSON.stringify(id)}).textContent`);
  const key = async (k, {shift = false, up = true} = {}) => {
    const mods = shift ? 8 : 0, code = {' ': 32, d: 68, D: 68}[k];
    await b.send('Input.dispatchKeyEvent', {type: 'keyDown', key: k, text: k, modifiers: mods, windowsVirtualKeyCode: code});
    if (up) await b.send('Input.dispatchKeyEvent', {type: 'keyUp', key: k, modifiers: mods, windowsVirtualKeyCode: code});
  };
  /** Click the wave area of a row at time t. */
  const clickRow = async (key, time) => {
    const p = await b.evaluate(`(() => { const c = document.getElementById('mt-canvas'); c.scrollIntoView({block: 'center'}); const r = c.getBoundingClientRect(), G = MT.geometry();
      const row = G.list.find(x => x.key === ${JSON.stringify(key)}), x = G.x0 + (${time} - MT.S.view[0]) / (MT.S.view[1] - MT.S.view[0]) * G.W;
      return {x: r.left + x, y: r.top + row.y + row.h / 2}; })()`);
    for (const type of ['mousePressed', 'mouseReleased']) await b.send('Input.dispatchMouseEvent', {type, button: 'left', clickCount: 1, ...p});
  };
  /** Share of the wave rows' pixels (not the pipeline) close to the difference colour. */
  const diffPixels = () => b.evaluate(`(() => {
    const c = document.getElementById('mt-canvas'), G = MT.geometry(), s = c.width / c.clientWidth;
    const d = MT.parse(getComputedStyle(document.getElementById('demo-frame')).getPropertyValue('--viewer-diff')).map(v => v * 255);
    const img = c.getContext('2d').getImageData(Math.round(G.x0 * s), Math.round(G.ruler * s), Math.round(G.W * s), Math.round((G.pipe.y - 6 - G.ruler) * s)).data;
    let n = 0; for (let i = 0; i < img.length; i += 4) if (Math.abs(img[i] - d[0]) < 8 && Math.abs(img[i + 1] - d[1]) < 8 && Math.abs(img[i + 2] - d[2]) < 8) n++;
    return n / (img.length / 4); })()`);

  // Parts of one system: landing.vtr and dram.fst combined, one root each, the missing address linked across them.
  let s = await state();
  assert.deepEqual([s.mode, s.traces, s.link, s.side], ['combine', ['base', 'dram'], '10000900', 'tree']);
  assert.equal(await b.evaluate(`[...document.querySelectorAll('#mt-chips .mt-trace__name')].map(e => e.textContent).join(',')`), 'landing,dram');
  assert.match(await b.evaluate(`document.querySelector('#mt-chips [data-id="dram"] .mt-trace__role').textContent`), /^FST ps · combined$/);
  assert.equal(await b.evaluate(`document.querySelectorAll('#mt-tree .mt-root').length`), 2);
  assert.equal(await text('mt-summary'), '2 traces on one timeline · ns and ps shown in ns · linked value 10000900: 3 in A, 2 in B');
  assert.equal(await b.evaluate(`document.getElementById('mt-align-group').hidden && document.getElementById('mt-tab-diff').disabled`), true);
  await clickRow('dram:lpddr_tb.ctrl.addr', 160);
  assert.equal((await state()).link, null, 'a second click on the linked value unlinks it');
  await clickRow('dram:lpddr_tb.ctrl.state', 164);
  assert.equal((await state()).link, 'RD');
  await b.click('#mt-tree [data-id="dram"][data-path="lpddr_tb.ctrl.cke"]');
  assert.ok(await b.evaluate(`MT.rows().some(r => r.key === 'dram:lpddr_tb.ctrl.cke')`), 'the hierarchy adds a row of that trace');

  // A regression: dram20 against the base at the same time, the L2 response selected and opened.
  await b.click('#mt-scenario [data-scenario="regress"]');
  s = await state();
  assert.deepEqual([s.mode, s.traces, s.focus, s.cursor, s.selected, s.side], ['compare', ['base', 'dram20'], 'dram20', 177, 'soc.l2.resp_valid', 'diff']);
  assert.match(await text('mt-summary'), /^15 of 16 signals differ · first at 177 ns$/);
  assert.equal(await b.evaluate(`document.querySelector('#mt-list .mt-diff').dataset.path`), 'soc.l2.resp_data');
  assert.ok(await diffPixels() > 0.002, 'difference bands drawn');
  await b.evaluate(`document.getElementById('mt-canvas').focus(); true`);
  await key('d');
  const next = await b.evaluate(`MT.diff('soc.l2.resp_valid', 'dram20').runs.map(r => r[0]).find(t => t > 177)`);
  assert.equal((await state()).cursor, next);
  await key('D', {shift: true});
  assert.equal((await state()).cursor, 177);
  await key(' ', {up: false});
  assert.equal((await state()).blink, true);
  await b.send('Input.dispatchKeyEvent', {type: 'keyUp', key: ' ', windowsVirtualKeyCode: 32});
  assert.equal((await state()).blink, false);
  await b.click('#mt-list [data-path="soc.l2.req_write"]');
  s = await state();
  assert.deepEqual([s.added, s.selected, s.cursor], [['soc.l2.req_write'], 'soc.l2.req_write', 195]);
  await b.click('#mt-chips [data-add="reset12"]');
  s = await state();
  assert.deepEqual([s.traces, s.focus], [['base', 'dram20', 'reset12'], 'reset12']);

  // A late reset: the suggestion appears, applying it places the run by its offset, and Tolerant leaves nothing.
  await b.click('#mt-scenario [data-scenario="reset"]');
  assert.equal(await b.evaluate(`document.getElementById('mt-suggest').hidden`), false);
  assert.equal(await text('mt-suggest-text'), 'B looks like A delayed by 16 ns: 59% of its early edges agree after that shift.');
  await b.click('#mt-suggest-apply');
  assert.equal((await state()).align.reset12, 'offset');
  assert.match(await text('mt-summary'), /^2 of 16 signals differ/);
  await b.click('#mt-rules [data-rules="tolerant"]');
  assert.match(await text('mt-summary'), /^No differences in 16 signals · \d+ hidden by rules$/);
  assert.equal(await diffPixels(), 0, 'no difference bands when the runs match');

  // Performance: aligned by instruction; the slip strip moves the view.
  await b.click('#mt-scenario [data-scenario="perf"]');
  s = await state();
  assert.equal(s.align.dram20, 'insn');
  assert.equal(await b.evaluate(`document.querySelector('#mt-chips [data-id="dram20"] .mt-trace__role').textContent`), 'VTR ns · by instruction');
  const before = s.view[0];
  await b.evaluate(`(() => { const c = document.getElementById('mt-canvas'), r = c.getBoundingClientRect(), G = MT.geometry();
    const x = r.left + G.x0 + 8 + 1200 / 1685 * (G.W - 16), y = r.top + G.pipe.slip + 24;
    c.dispatchEvent(new PointerEvent('pointerdown', {clientX: x, clientY: y, bubbles: true, pointerId: 1})); return true; })()`);
  s = await state();
  assert.ok(Math.abs(s.cursor - 1200) < 2 && s.view[0] !== before, 'clicking the slip curve moves there');

  // Three runs: sub-rows per trace, a chip click changes the focus.
  await b.click('#mt-scenario [data-scenario="three"]');
  s = await state();
  assert.deepEqual([s.traces, s.align.reset12, s.rules], [['base', 'dram20', 'reset12'], 'offset', 'tolerant']);
  assert.equal(await b.evaluate(`MT.rows().filter(r => r.type === 'trace' && r.path === 'soc.cpu0.pc').length`), 3);
  await b.click('#mt-chips [data-id="reset12"]');
  assert.equal((await state()).focus, 'reset12');
  assert.match(await text('mt-summary'), /^No differences/);
  assert.deepEqual(b.exceptions, []);
});

test('each stage picture shows only its stage and the ones before', {timeout: 60000}, async t => {
  const b = await open(t);
  /** Share of a stage canvas's pixels close to the difference colour. */
  const diffShare = n => b.evaluate(`(() => {
    const c = document.getElementById('stage-${n}-canvas');
    const d = MT.parse(getComputedStyle(c.closest('figure')).getPropertyValue('--viewer-diff')).map(v => v * 255);
    const img = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
    let k = 0; for (let i = 0; i < img.length; i += 4) if (Math.abs(img[i] - d[0]) < 8 && Math.abs(img[i + 1] - d[1]) < 8 && Math.abs(img[i + 2] - d[2]) < 8) k++;
    return k / (img.length / 4); })()`);
  for (const n of [1, 2, 3]) assert.equal(await diffShare(n), 0, `stage ${n} draws no differences`);
  for (const n of [4, 5, 6, 7, 8]) assert.ok(await diffShare(n) > 0.001, `stage ${n} draws differences`);
  const features = await b.evaluate(`[1, 2, 3, 4, 5, 6, 7, 8].map(n => [...MT.featuresUpTo(n)].at(-1))`);
  assert.deepEqual(features, ['tags', 'links', 'pairs', 'diff', 'place', 'perf', 'diverge', 'many']);
  // Every red circle finds the element it points at, and the text lists as many items as the picture circles.
  assert.deepEqual(await b.evaluate(`[1, 2, 3, 4, 5, 6, 7, 8].map(n => MT.renderStage(n))`), Array(8).fill([]));
  for (let n = 1; n <= 8; n++) {
    const [items, marks] = await b.evaluate(`[document.querySelectorAll('#stage-${n} .mt-adds > li').length, MT.STAGE_FIGS[${n - 1}].marks.length]`);
    assert.equal(items, marks, `stage ${n}: one numbered item per circle`);
  }
  // What each picture draws follows the plan: letters on rows only while traces are combined, the Differences tab
  // from stage 7, the second ruler once placement lands in stage 5, the slip curve with stage 6.
  const drawn = await b.evaluate('MT.STAGE_DRAWN');
  const has = (n, name) => drawn[n].includes(name);
  assert.ok(has(1, 'tags') && has(1, 'root-dram') && has(1, 'chips') && !has(1, 'bar'));
  assert.ok(has(2, 'link-base') && has(2, 'link-dram'));
  assert.ok(has(3, 'subrows') && has(3, 'root-merged') && !has(3, 'band') && !has(3, 'tags'));
  assert.ok(has(4, 'band') && has(4, 'bar') && !has(4, 'align') && !has(4, 'ruler2'));
  assert.ok(has(5, 'align') && has(5, 'ruler2') && !has(5, 'align-insn') && !has(5, 'slip'));
  assert.ok(has(6, 'align-insn') && has(6, 'slip') && !has(6, 'diff-tab'));
  assert.ok(has(7, 'diff-tab') && has(7, 'ranked'));
  // The main demo is unchanged by painting the stages.
  assert.equal(await b.evaluate('MT.S.scenario'), 'combine');
  assert.deepEqual(b.exceptions, []);
});

test('both themes draw the canvases with the viewer tokens', {timeout: 60000}, async t => {
  const b = await open(t);
  const probe = () => b.evaluate(`(() => {
    const c = document.getElementById('mt-canvas'), s = c.width / c.clientWidth, G = MT.geometry(), row = G.list.find(r => r.key === 'dram:lpddr_tb.ctrl.ecc_err');
    const px = [...c.getContext('2d').getImageData(Math.round((G.x0 + G.W - 4) * s), Math.round((row.y + 3) * s), 1, 1).data.slice(0, 3)];
    const k = MT.skin(document.getElementById('demo-frame'));
    return {px, canvas: k.canvas.slice(0, 3).map(v => Math.round(v * 255)), diff: MT.contrast(k.diff, k.canvas)}; })()`);
  for (const mode of ['dark', 'light']) {
    await b.click(`#page-theme [data-mode="${mode}"]`);
    assert.equal(await b.evaluate('document.documentElement.dataset.theme'), mode);
    const p = await probe();
    assert.deepEqual(p.px, p.canvas, `${mode} canvas`);
    assert.ok(p.diff >= 4.5, `${mode} difference colour ${p.diff.toFixed(2)}:1`);
  }
  for (const pair of ['dram20', 'reset12']) for (const mode of ['time', 'offset', 'insn']) {
    await b.click(`#al-pair [data-pair="${pair}"]`);
    await b.click(`#al-mode [data-mode="${mode}"]`);
    assert.deepEqual(await b.evaluate(`[MT.AL.pair, MT.AL.mode]`), [pair, mode]);
  }
  assert.deepEqual(b.exceptions, []);
});
