// node --test docs/tests/recent_sessions.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../recent_sessions.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const state = b => b.evaluate('RS.state()');
async function open(width = 1280) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 900, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.evaluate(`document.getElementById('win').focus()`);
  return b;
}
async function key(b, key, modifiers = 0) {
  const code = key.length === 1 && /\d/.test(key) ? 'Digit' + key : '';
  await b.send('Input.dispatchKeyEvent', {type: 'rawKeyDown', key, code, modifiers});
  await b.send('Input.dispatchKeyEvent', {type: 'keyUp', key, code, modifiers});
}

for (const width of [1280, 390]) test(`page layout, self-test and accessibility at ${width}px`, {timeout: 30000}, async t => {
  const b = await open(width); t.after(() => b.close());
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /selftest: all passed/, selftest);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links resolve');
  const unnamed = await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`);
  assert.equal(unnamed, 0, 'buttons have names');
  assert.equal(await b.evaluate('document.querySelectorAll(".callout,.card,.badge").length'), 0, 'no callout boxes');
  const rows = await b.evaluate(`[...document.querySelectorAll('#page [role=option]')].map(li => li.getBoundingClientRect().height)`);
  assert.equal(rows.length, 8, 'eight entries shown');
  assert.ok(rows.every(h => h > 20 && h < 40), `rows stay one line: ${rows}`);
  assert.equal(await b.evaluate(`document.querySelectorAll('#page [aria-selected=true]').length`), 1);
  assert.ok(html.length < 40000, `page stays short: ${html.length} bytes`);
  assert.deepEqual(b.exceptions, []);
});

test('Enter reopens the last session; arrows, digits and Delete work', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  let s = await state(b);
  assert.deepEqual([s.sel, s.names[0]], [0, 'c910_coremark.vtr']);
  await key(b, 'ArrowDown'); await key(b, 'ArrowDown');
  await key(b, 'Enter');
  s = await state(b);
  assert.equal(s.names[0], 'top.vtr', 'opened entry moves to the top');
  assert.equal(s.names.filter(n => n === 'top.vtr').length, 2, 'the other top.vtr stays; no duplicates');
  assert.equal(s.total, 8);
  assert.equal(s.sel, 0);
  await key(b, '3');
  s = await state(b);
  assert.equal(s.names[0], 'chi_hang', 'digit opens by position');
  assert.match(await b.evaluate(`document.getElementById('vstatus').textContent`), /xs_dualcore\.vtr with workspace chi_hang/);
  await key(b, 'End'); await key(b, 'Delete');
  s = await state(b);
  assert.equal(s.total, 7);
  assert.ok(!s.names.includes('o3_pipeline'), 'Delete removed the selected entry');
});

test('missing files stay listed and report an error', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  const gone = await b.evaluate(`[...document.querySelectorAll('#page li.gone')].map(li => li.textContent)`);
  assert.equal(gone.length, 1); assert.match(gone[0], /picorv32\.fst.*Not found/);
  await key(b, '4');
  const s = await state(b);
  assert.match(s.error, /picorv32\.fst: no such file/);
  assert.equal(s.total, 8, 'entry kept');
  assert.equal(await b.evaluate(`document.querySelectorAll('#page [role=alert]').length`), 1);
  await b.click('[data-rm="3"]');
  assert.ok(!(await state(b)).names.includes('picorv32.fst'), '× removes it');
});

test('first run keeps the hints; long history expands in place', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  assert.equal(await b.evaluate(`document.querySelectorAll('#page .hints').length`), 0, 'list replaces the hints');
  await b.click('[data-scn="first"]');
  assert.equal(await b.evaluate(`document.querySelectorAll('#page [role=option]').length`), 0);
  assert.match(await b.evaluate(`document.querySelector('#page .hints').textContent`), /Open a trace⌘O/);
  await b.click('[data-scn="long"]');
  assert.equal((await state(b)).names.length, 8);
  assert.match(await b.evaluate(`document.querySelector('[data-act=more]').textContent`), /Show all 14/);
  await b.click('[data-act="more"]');
  assert.equal((await state(b)).names.length, 14);
  await b.click('#page li[data-i="12"]');
  assert.equal((await state(b)).names[0], 'axi_burst.vtr', 'click opens an entry past the first eight');
});
