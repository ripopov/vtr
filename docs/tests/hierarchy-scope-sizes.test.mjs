// node --test --test-concurrency=1 docs/tests/hierarchy-scope-sizes.test.mjs
// The scope-sizes design: the page's streaming count (preorder, offline LCA,
// +1/−1) equals a brute-force walk for every subset of the fragment's
// signals; the demo toggles signals and shows the method; the staged plan's
// pictures ring every numbered addition. The design-system guardrails cover
// the page's lint, contrast and 360px layout.
import assert from 'node:assert/strict';
import {join} from 'node:path';
import {test} from 'node:test';
import {browser, root} from './design-system-lib.mjs';

const page = join(root, 'docs/hierarchy-scope-sizes.html');

async function open(t, width = 1280) {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page, {width});
  await b.wait('window.ready === true', 30000);
  return b;
}

test('the streaming count equals brute force for every subset of signals', {timeout: 60000}, async t => {
  const b = await open(t);
  const bad = await b.evaluate(`(() => { const out = []; for (let m = 0; m < 128; m++) { const a = SIZES.census(m), z = SIZES.brute(m); if (a.join() !== z.join()) out.push([m, a, z]); } return out; })()`);
  assert.deepEqual(bad, []);
  // All seven signals: top holds all of them, the adder four, each cell three.
  assert.deepEqual(await b.evaluate('SIZES.census(127)'), [7, 4, 3, 3, 5, 3, 3]);
  assert.deepEqual(b.exceptions, []);
});

test('the demo leaves signals out and shows the method', {timeout: 60000}, async t => {
  const b = await open(t);
  const rows = async () => (await b.evaluate('SIZES.state()')).rows;
  assert.deepEqual((await rows())[0], ['28', '7']);
  // Leave clk out: top keeps 6 of 7, the register block 4 of 5.
  await b.click('#chips [data-g="0"]');
  await b.wait(`SIZES.state().active[0] === false`);
  assert.deepEqual((await rows())[0], ['28', '6 of 7']);
  assert.deepEqual((await rows())[4], ['11', '4 of 5']);
  await b.click('#chips [data-g="0"]');
  await b.wait(`SIZES.state().active[0] === true`);
  // The method: weights sum over each subtree to its size, and the readout names the LCAs.
  await b.click('#mode [data-mode="method"]');
  await b.click('#chips [data-g="3"]'); await b.click('#chips [data-g="3"]');
  await b.wait(`SIZES.state().mode === 'method'`);
  const r = await rows();
  assert.equal(r.length, 7);
  assert.equal(r.reduce((a, x) => a + Number(x[2]), 0), 7, 'the weights of the whole tree sum to its size');
  assert.match(await b.evaluate(`document.getElementById('readout').textContent`), /s is held by top, u_add, U1, u_reg, F0.*LCA\(U1, u_reg\) = top/);
  assert.deepEqual(b.exceptions, []);
});

test('the plan pictures ring every numbered addition', {timeout: 60000}, async t => {
  for (const width of [1280, 360]) {
    const b = await open(t, width);
    const plan = await b.evaluate('SIZES.plan()');
    assert.deepEqual(plan.map(s => s.id), ['shot-1', 'shot-3', 'shot-5']);
    for (const shot of plan) {
      const stage = shot.id.replace('shot-', 'stage-');
      const items = await b.evaluate(`document.querySelectorAll('#${stage} .a-adds > li').length`);
      assert.deepEqual(shot.marks, Array.from({length: items}, (_, k) => String(k + 1)), `${stage} marks`);
      assert.deepEqual(shot.rings.map(r => r.n).sort(), shot.marks, `${stage} rings`);
      assert.ok(shot.rings.every(r => r.inside), `${stage} rings stay inside at ${width}px`);
    }
    assert.equal(await b.evaluate(`document.querySelectorAll('#plan .a-stagecard').length`), 6);
    assert.equal(await b.evaluate(`[...document.querySelectorAll('#plan tbody a')].filter(a => document.querySelector(a.getAttribute('href'))).length`), 6);
    assert.equal(await b.evaluate('document.documentElement.scrollWidth'), width, 'no horizontal page scroll');
    assert.deepEqual(b.exceptions, []);
  }
});
