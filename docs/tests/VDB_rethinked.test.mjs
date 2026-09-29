// node --test --test-concurrency=1 docs/tests/VDB_rethinked.test.mjs
// The VDB vision page: the demo's counts are the replay of C910's preg lifecycle
// machine (bench/workloads/c910/fsm_replay.py), its questions compute their
// answers from that data, its interactions work, the diagram's labels do not
// collide, and the plan and references are complete. The design-system
// guardrails cover the page's lint, contrast and 360px layout.
import assert from 'node:assert/strict';
import {join} from 'node:path';
import {test} from 'node:test';
import {browser, root} from './design-system-lib.mjs';

const page = join(root, 'docs/VDB_rethinked.html');

async function open(t, width = 1280) {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page, {width});
  await b.wait('window.ready === true', 30000);
  return b;
}

test('the demo shows the replayed transitions of all 95 entries', {timeout: 60000}, async t => {
  const b = await open(t);
  const totals = await b.evaluate('VDB.totals()');
  assert.equal(Object.values(totals).reduce((a, n) => a + n, 0), 1181073, 'every recorded change is attributed to a source line');
  assert.deepEqual(totals, {r270: 126, r272: 62, e288: 311788, e292: 17218, e294: 294566, e298: 29868, e300: 2520,
    e302: 0, e304: 262166, e308: 261573, e310: 593, e314: 593});
  // Arrivals equal departures for every state but the one entered by the initial reset.
  const flow = await b.evaluate(`(() => { const t = VDB.totals(), io = {};
    const edges = {r270: ['reset', 0], r272: ['sync', 3], e288: [0, 1], e292: [1, 0], e294: [1, 2], e298: [2, 0], e300: [2, 0], e302: [2, 4], e304: [2, 3], e308: [3, 0], e310: [3, 4], e314: [4, 0]};
    for (const [id, [a, z]] of Object.entries(edges)) { io[a] = (io[a] ?? 0) - t[id]; io[z] = (io[z] ?? 0) + t[id]; }
    return io; })()`);
  assert.ok(Math.abs(flow[1]) <= 95 && Math.abs(flow[2]) <= 95 && Math.abs(flow[4]) <= 95, JSON.stringify(flow));
  const s = await b.evaluate('VDB.state()');
  assert.match(s.readout, /All 95 entries: 1,181,073 state changes in the run/);
  assert.equal(s.labels.filter(l => l === 'never').length, 1, 'one transition is never taken');
  assert.equal((await b.evaluate('VDB.lanes()')).length, 95);
  assert.deepEqual(b.exceptions, []);
});

test('choosing an entry, a transition and the facts pane', {timeout: 60000}, async t => {
  const b = await open(t);
  await b.evaluate(`document.getElementById('inst').value = '15'; document.getElementById('inst').dispatchEvent(new Event('change'))`);
  let s = await b.evaluate('VDB.state()');
  assert.equal(s.inst, 15);
  assert.ok(s.occupancy[3] / s.occupancy.reduce((a, n) => a + n, 0) > 0.99, 'preg15 spends the run in RETIRE');
  await b.click('#fsm [data-edge="e302"] .f-count');
  s = await b.evaluate('VDB.state()');
  assert.equal(s.edge, 'e302');
  assert.match(s.readout, /preg15: ALLOC → RELEASE 0 times · lines 302, 303/);
  assert.deepEqual(await b.evaluate(`[...document.querySelectorAll('#src .is-hi')].map(e => e.dataset.line)`), ['302', '303']);
  await b.click('#side [data-mode="facts"]');
  assert.equal(await b.evaluate(`document.getElementById('facts').hidden`), false);
  assert.deepEqual(await b.evaluate(`[...document.querySelectorAll('#facts [data-layer]')].map(e => e.dataset.layer)`),
    ['Compiled', 'Derived', 'Bound', 'Trace-derived', 'Pack', 'Note']);
  const hover = await b.evaluate('VDB.hover(1, 159466)');
  assert.equal(typeof hover.s, 'number');
  assert.deepEqual(b.exceptions, []);
});

test('the questions compute their answers from the recording', {timeout: 60000}, async t => {
  const b = await open(t);
  const ask = async id => { await b.click(`#questions [data-q="${id}"]`); return (await b.evaluate('VDB.state()')).answer; };
  assert.match(await ask('never'), /never taken in any of the 95 entries: ALLOC → RELEASE \(lines 302–303\).*593 times, in \d+ of the 95 entries/);
  const preg15 = await ask('preg15');
  assert.match(preg15, /enters RETIRE at t = 4,668 through line 304.*499,471 of 504,139 time units, 99\.1%/);
  assert.match(preg15, /last high from 4,576 to 4,578/);
  assert.equal((await b.evaluate('VDB.state()')).notes, 1, 'the finding is saved as a note');
  assert.match(await b.evaluate(`document.querySelector('#facts [data-layer="Note"]').textContent`), /Status: hypothesis/);
  assert.match(await ask('flush'), /flushes undo 47,086, 15\.1%: 17,218 while waiting .* 29,868 after allocation .* 2,520 ALLOC → DEALLOC changes are releases after write-back/);
  assert.match(await ask('flush'), /the flush at t = 159,466 frees \d+ entries at once/);
  assert.match(await ask('agree'), /predicts all 1,181,073 recorded changes .* over 3,308,480 edges.*62 edges, at t = 14 and 54/);
  // Every citation uses a link form the page proposes.
  assert.ok((await b.evaluate(`[...document.querySelectorAll('#answer .d-cite')].map(c => c.dataset.kind)`)).every(k => ['vdb', 'src', 'vtr'].includes(k)));
  await b.click('#reset');
  const s = await b.evaluate('VDB.state()');
  assert.deepEqual([s.inst, s.edge, s.question, s.notes], ['all', null, null, 0]);
  assert.deepEqual(b.exceptions, []);
});

test('the diagram labels stay apart and inside the drawing', {timeout: 60000}, async t => {
  for (const width of [1280, 360]) {
    const b = await open(t, width);
    const boxes = await b.evaluate(`(() => { const svg = document.getElementById('fsm'), r = svg.getBoundingClientRect();
      return [...svg.querySelectorAll('text')].map(e => { const q = e.getBoundingClientRect();
        return {text: e.textContent, l: q.left, t: q.top, r: q.right, b: q.bottom, inside: q.left >= r.left - 0.5 && q.right <= r.right + 0.5 && q.top >= r.top - 0.5 && q.bottom <= r.bottom + 0.5}; }); })()`);
    assert.ok(boxes.length >= 24, `${boxes.length} labels`);
    assert.deepEqual(boxes.filter(x => !x.inside).map(x => x.text), [], `labels inside at ${width}px`);
    const overlaps = [];
    for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i], z = boxes[j];
      if (a.l < z.r - 1 && z.l < a.r - 1 && a.t < z.b - 1 && z.t < a.b - 1) overlaps.push(`${a.text} / ${z.text}`);
    }
    assert.deepEqual(overlaps, [], `no overlapping labels at ${width}px`);
    assert.equal(await b.evaluate('document.documentElement.scrollWidth'), width, 'no horizontal page scroll');
    assert.deepEqual(b.exceptions, []);
  }
});

test('the plan, citations and references are complete', {timeout: 60000}, async t => {
  const b = await open(t);
  assert.equal(await b.evaluate(`document.querySelectorAll('#plan .a-stagecard').length`), 15);
  assert.equal(await b.evaluate(`[...document.querySelectorAll('#plan tbody a')].filter(a => document.querySelector(a.getAttribute('href'))).length`), 15);
  const refs = await b.evaluate(`[...document.querySelectorAll('#references li')].map(li => li.id)`);
  assert.deepEqual(refs, Array.from({length: refs.length}, (_, i) => `r${i + 1}`));
  const cited = await b.evaluate(`[...new Set([...document.querySelectorAll('.a-cite a')].map(a => a.getAttribute('href')))]`);
  assert.ok(cited.every(h => refs.includes(h.slice(1))), 'every citation resolves');
  // Every reference is cited, directly or inside a cited range.
  const ranges = await b.evaluate(`[...document.querySelectorAll('.a-cite')].flatMap(c => { const n = [...c.querySelectorAll('a')].map(a => +a.getAttribute('href').slice(2));
    return c.textContent.includes('–') ? Array.from({length: n[1] - n[0] + 1}, (_, k) => n[0] + k) : n; })`);
  assert.deepEqual(refs.filter(r => !ranges.includes(+r.slice(1))), []);
  // Links to sibling pages point at files that exist.
  const local = await b.evaluate(`[...new Set([...document.querySelectorAll('a[href]')].map(a => a.getAttribute('href')).filter(h => !h.startsWith('#') && !/^[a-z]+:/.test(h)))]`);
  for (const href of local) {
    const ok = await b.evaluate(`fetch(${JSON.stringify(href.split('#')[0])}, {method: 'HEAD'}).then(r => r.ok)`);
    assert.ok(ok, href);
  }
  assert.deepEqual(b.exceptions, []);
});
