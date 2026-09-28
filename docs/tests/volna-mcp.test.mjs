// node --test docs/tests/volna-mcp.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../volna-mcp.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const state = b => b.evaluate('AG.state');
async function open(width = 1280) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 900, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.evaluate('AG.speed = 8');
  return b;
}
async function guide(b, name) {
  await b.click(`[data-guide="${name}"]`);
  await b.wait('AG.state.busy === true || document.querySelector(\'[data-guide="' + name + '"]\').classList.contains("on")');
  await b.wait('AG.state.busy === false');
}
// first time a signal takes a value, from the trace the page generated
const firstAt = (b, path, value, after = -1) => b.evaluate(`AG.SIG[${JSON.stringify(path)}].ch.find(([t, x]) => t > ${after} && x === ${value})[0]`);
async function canvasPoint(b, {row, time}) {
  return b.evaluate(`(() => { const cv = document.getElementById('wv'); cv.scrollIntoView({block: 'center', behavior: 'instant'});
    const r = cv.getBoundingClientRect(), name = cv.clientWidth < 520 ? 118 : 170;
    const x = ${time === undefined} ? 20 : name + (${time ?? 0} - AG.state.view[0]) / (AG.state.view[1] - AG.state.view[0]) * (cv.clientWidth - name - 8);
    const y = 22 + 4 + ${row} * 24 + 12;
    return {x: r.left + x, y: r.top + y}; })()`);
}
async function click(b, p) {
  for (const type of ['mousePressed', 'mouseReleased']) await b.send('Input.dispatchMouseEvent', {type, x: p.x, y: p.y, button: 'left', clickCount: 1});
}
async function key(b, key, modifiers = 0) {
  for (const type of ['rawKeyDown', 'keyUp']) await b.send('Input.dispatchKeyEvent', {type, key, modifiers});
}

for (const width of [1280, 390]) test(`page layout, self-test, links and accessibility at ${width}px`, {timeout: 30000}, async t => {
  const b = await open(width); t.after(() => b.close());
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /selftest: all passed \(\d+ checks\)/, selftest);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links and citations resolve');
  const uncited = await b.evaluate(`[...document.querySelectorAll('ol.refs li[id]')].filter(li => !document.querySelector('sup.r a[href="#' + li.id + '"]')).map(li => li.id)`);
  assert.deepEqual(uncited, [], 'every reference is cited');
  const numbers = await b.evaluate(`[...document.querySelectorAll('sup.r a')].every(a => a.textContent === '[' + ([...document.querySelectorAll('ol.refs li[id]')].findIndex(li => '#' + li.id === a.hash) + 1) + ']')`);
  assert.equal(numbers, true, 'citation numbers match the reference list');
  assert.equal(await b.evaluate(`document.body.textContent.includes('[[')`), false, 'no unresolved citation markers');
  const unnamed = await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`);
  assert.equal(unnamed, 0, 'buttons have names');
  assert.equal(await b.evaluate('document.querySelectorAll(".callout,.card,.badge").length'), 0, 'no callout boxes');
  assert.equal(await b.evaluate('document.querySelector("#wv").clientWidth > 300'), true, 'waves mock is visible');
  assert.equal(await b.evaluate('document.getElementById("demo").compareDocumentPosition(document.getElementById("summary")) & Node.DOCUMENT_POSITION_FOLLOWING'), 4, 'the demo follows the hero');
  const tools = await b.evaluate(`[...document.querySelectorAll('#tool-table tbody tr td:first-child code')].map(e => e.textContent)`);
  assert.equal(tools.length, 15, 'fifteen tools in the table');
  assert.match(await b.evaluate('document.querySelector(".host-h .sim").textContent'), /simulated/i, 'the conversation is labelled simulated');
  await b.click('#theme');
  assert.match(await b.evaluate('document.documentElement.dataset.theme'), /^(light|dark)$/);
  assert.deepEqual(b.exceptions, []);
});

test('the trace has the bug the page describes', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  const full = await firstAt(b, 'top.dut.fifo.count', 8), drop = await firstAt(b, 'top.dut.fifo.drop', 1);
  assert.equal(drop, full, 'the write in the cycle the FIFO fills is dropped');
  assert.equal(await b.evaluate(`AG.SIG['top.dut.in_ready'].ch.find(([t, x]) => t > ${full} && x === 0)[0]`), full + 10, 'in_ready falls one cycle late');
  assert.equal(await b.evaluate(`AG.SIG['top.dut.fifo.drop'].ch.filter(([, x]) => x === 1).length`), 1, 'one drop in the whole trace');
  const snapshot = await b.evaluate(`[...document.querySelectorAll('#examples pre.code')].map(p => p.textContent).join('\\n')`);
  assert.match(snapshot, new RegExp(`“drop” ${drop} ns`), 'the example snapshot quotes the drop time of the demo trace');
  assert.deepEqual(b.exceptions, []);
});

test('guided scenarios end in the states their text describes', {timeout: 90000}, async t => {
  const b = await open(); t.after(() => b.close());
  const drop = await firstAt(b, 'top.dut.fifo.drop', 1);

  await guide(b, 'orient');
  let s = await state(b);
  assert.match(s.log.at(-1), /^agent:.*5 signals.*1000 ns/, 'the agent describes the live view');
  assert.deepEqual(s.undo, [], 'reading changes nothing');

  await guide(b, 'deictic');
  s = await state(b);
  const at = s.rows.indexOf('top.dut.fifo.count');
  assert.deepEqual(s.rows.slice(at, at + 4), ['top.dut.fifo.count', 'top.dut.fifo.full', 'top.dut.fifo.drop', 'top.dut.out_valid'], 'rows land under the selection');
  assert.deepEqual(s.selected, ['top.dut.fifo.count'], 'the user selection is kept');
  assert.deepEqual(s.undo, ['Agent: Add 3 signals'], 'one agent step');
  assert.match(s.log.at(-1), new RegExp(`drop pulses at ${drop} ns`), 'the explanation quotes the drop from a tool result');

  await guide(b, 'find');
  s = await state(b);
  assert.deepEqual(s.markers, [{id: 1, time: drop, label: 'drop'}]);
  assert.equal(s.cursor, drop);
  assert.deepEqual(s.view, [drop - 150, drop + 150]);
  assert.equal(s.undo.at(-1), 'Agent: Add marker 1', 'navigation adds no step');

  await guide(b, 'ambiguous');
  s = await state(b);
  assert.ok(s.log.some(l => /^tool:show.*ambiguous/.test(l)), 'the ambiguous name fails loudly');
  assert.ok(s.log.some(l => /^agent:.*in_ready and out_ready\. Which one\?/.test(l)), 'the agent asks');

  await guide(b, 'together');
  s = await state(b);
  assert.ok(s.log.some(l => /^tool:show.*busy/.test(l)), 'the edit waits while the user drags');
  assert.deepEqual(s.undo.slice(-2), ['Move 1 row', 'Agent: Add 2 signals'], 'the user and the agent get separate steps, in order');
  assert.equal(s.rows[0], 'top.dut.in_valid', 'the user drag landed');
  assert.ok(s.rows.includes('top.dut.in_data') && s.rows.includes('top.dut.out_data'));

  await guide(b, 'undo');
  s = await state(b);
  assert.equal(s.undo.at(-1), 'Move 1 row', 'only the agent step is undone');
  assert.deepEqual(s.redo, ['Agent: Add 2 signals']);
  assert.ok(!s.rows.includes('top.dut.in_data'));

  await guide(b, 'undo');
  s = await state(b);
  assert.equal(s.undo.at(-1), 'Move 1 row', 'the agent refuses to undo the user step');
  assert.match(s.log.at(-1), /newest step is the user's/);

  await guide(b, 'long');
  s = await state(b);
  const empty = await firstAt(b, 'top.dut.fifo.empty', 1, 1300);
  assert.ok(s.log.some(l => /^tool:wait.*still working/.test(l)) || s.log.some(l => /^tool:find.*task/.test(l)), 'the load becomes a task');
  assert.match(s.log.at(-1), new RegExp(`fifo.empty is next 1 at ${empty} ns`));
  assert.deepEqual(b.exceptions, []);
});

test('direct interaction is what the agent sees, and undo is shared', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  await click(b, await canvasPoint(b, {row: 3}));
  let s = await state(b);
  assert.deepEqual(s.selected, ['top.dut.fifo.count'], 'clicking a name selects it');
  await click(b, await canvasPoint(b, {row: 2, time: 1420}));
  s = await state(b);
  assert.ok(Math.abs(s.cursor - 1420) <= 10, `clicking the waves moves the cursor (${s.cursor})`);
  await b.click('#b-sees');
  await b.wait('AG.state.busy === false && AG.state.log.some(l => l.startsWith("tool:get_view"))');
  const snap = await b.evaluate(`document.querySelector('#log details:last-of-type pre').textContent`);
  assert.match(snap, new RegExp(`cursor ${s.cursor} ns`), 'the snapshot has the user cursor');
  assert.match(snap, /\* top\.dut\.fifo\.count\s+udec 8/, 'the snapshot marks the selection and shows its value');
  assert.match(snap, /selection: top\.dut\.fifo\.count/);

  await b.evaluate(`document.getElementById('pv').focus()`);
  await key(b, 'Delete');
  s = await state(b);
  assert.equal(s.rows.length, 4); assert.deepEqual(s.undo, ['Remove 1 row']);
  await key(b, 'z', 2);
  s = await state(b);
  assert.equal(s.rows.length, 5, 'Ctrl+Z restores the row'); assert.deepEqual(s.redo, ['Remove 1 row']);

  await b.click('#b-pause');
  const r = await b.evaluate(`AG.call(AG.ui.v, 'markers', {op: 'add', time: 10}).then(r => r.error.code)`);
  assert.equal(r, 'paused', 'a paused agent cannot edit');
  const read = await b.evaluate(`AG.call(AG.ui.v, 'get_view').then(r => r.data.agent)`);
  assert.equal(read, 'paused', 'but it can still read');
  assert.match(await b.evaluate('document.getElementById("agenttext").textContent'), /paused/i);
  assert.deepEqual(b.exceptions, []);
});
