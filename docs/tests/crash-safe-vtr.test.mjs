// node --test --test-concurrency=1 docs/tests/crash-safe-vtr.test.mjs
// The crash-safe VTR design: the demo's embedded outcomes are exactly the
// measured ones in bench/results/crashlab.json; every ending and writer renders
// a consistent readout, file bar and event lanes; the results table covers
// every ending; the plan's table and stage cards agree; the page fits 360px.
// The design-system guardrails cover the page's lint, contrast and focus rings.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {join} from 'node:path';
import {test} from 'node:test';
import {browser, root} from './design-system-lib.mjs';

const page = join(root, 'docs/crash-safe-vtr.html');

async function open(t, width = 1280) {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page, {width});
  await b.wait('window.ready === true', 30000);
  return b;
}

test('the demo shows exactly the measured outcomes', {timeout: 60000}, async t => {
  const measured = JSON.parse(await readFile(join(root, 'bench/results/crashlab.json'), 'utf8'));
  assert.deepEqual(measured.failures, [], 'the crash lab run behind the page passed');
  const expected = {};
  for (const s of measured.scenarios) {
    expected[s.id] = {};
    for (const [cfg, r] of Object.entries(s.results))
      expected[s.id][cfg] = [r.status, r.emitted_changes, r.file.changes, r.emitted_logs, r.file.logs, r.file.recovered, r.rescue_ms, r.close_ms];
  }
  const b = await open(t);
  assert.deepEqual(await b.evaluate('CRASH.data'), expected);
  assert.deepEqual(b.exceptions, []);
});

test('every ending and writer renders a consistent run', {timeout: 60000}, async t => {
  const b = await open(t);
  const data = await b.evaluate('CRASH.data');
  for (const [scenario, configs] of Object.entries(data)) {
    for (const [config, r] of Object.entries(configs)) {
      await b.evaluate(`CRASH.select(${JSON.stringify(scenario)}, ${JSON.stringify(config)})`);
      const s = await b.evaluate('CRASH.state()');
      const complete = r[5] === 0;
      assert.equal(s.config, config, `${scenario}: ${config} is selectable`);
      assert.deepEqual(s.kept, [true, complete, complete, complete], `${scenario}/${config}: file bar`);
      assert.match(s.readout, complete ? /complete file/ : /recovered by scanning/, `${scenario}/${config}: readout`);
      assert.ok(s.readout.includes(r[2].toLocaleString('en-US')), `${scenario}/${config}: kept changes shown`);
      // The run starts with the encoder's state and ends in the kernel, except exit(), which ends on the simulation thread.
      assert.equal(s.events[0], 'enc');
      assert.equal(s.events.at(-1), scenario === 'exit' ? 'sim' : 'kernel', `${scenario}/${config}: last event`);
      assert.equal(s.events.includes('rescue'), !['none', undefined].includes(config) && !['term', 'exit', 'kill', 'noalt'].includes(scenario),
        `${scenario}/${config}: the rescue thread acts exactly when a crash is handled`);
    }
    assert.deepEqual((await b.evaluate('CRASH.state()')).disabled, 'none' in configs ? [] : ['none']);
  }
  // Clicking drives the same state.
  await b.click('#scenario [data-scenario="heaplock"]');
  await b.click('#config [data-config="private"]');
  const s = await b.evaluate('CRASH.state()');
  assert.equal(s.scenario, 'heaplock');
  assert.match(s.readout, /complete file/);
  assert.equal(await b.evaluate(`document.querySelectorAll('#results tbody tr').length`), Object.keys(data).length);
  assert.deepEqual(b.exceptions, []);
});

test('the plan is complete and the page fits a phone', {timeout: 60000}, async t => {
  for (const width of [1280, 360]) {
    const b = await open(t, width);
    const cards = await b.evaluate(`document.querySelectorAll('#plan .a-stagecard').length`);
    assert.equal(cards, 8);
    assert.equal(await b.evaluate(`[...document.querySelectorAll('#plan tbody a')].filter(a => document.querySelector(a.getAttribute('href'))).length`), cards);
    assert.equal(await b.evaluate(`document.querySelectorAll('#design .a-rules > li').length`), 12);
    assert.equal(await b.evaluate('document.documentElement.scrollWidth'), width, 'no horizontal page scroll');
    assert.deepEqual(b.exceptions, []);
  }
});
