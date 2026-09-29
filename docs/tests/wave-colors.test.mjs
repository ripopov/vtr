// node --test docs/tests/wave-colors.test.mjs
// The wave colours proposal: the demo's right-click menu applies a named colour
// to the selection, groups pass it down, X keeps its colour, and undo and redo
// restore it. The design-system guardrails cover the page's lint, contrast and
// 360px layout.
import assert from 'node:assert/strict';
import {join} from 'node:path';
import {test} from 'node:test';
import {browser, root} from './design-system-lib.mjs';

const page = join(root, 'docs/wave-colors.html');
const START = {clk: 'default', axi: 'cyan', valid: 'cyan', data: 'cyan', resp: 'pink', err: 'default', temp: 'default'};

async function open(t) {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page);
  await b.wait('window.ready === true');
  return b;
}
const fire = (b, sel, event) => b.evaluate(`document.querySelector(${JSON.stringify(sel)}).dispatchEvent(${event})`);
const row = id => `.c-name[data-id="${id}"]`;
const select = (b, id, add = false) => fire(b, row(id), `new MouseEvent('click', {bubbles: true, ctrlKey: ${add}})`);
const inks = b => b.evaluate('WC.inks()');
const stroke = (b, id, part = 'c-ink') => b.evaluate(`getComputedStyle(document.querySelector('.c-row[data-id="${id}"] .${part}')).stroke`);
const key = (b, sel, k, extra = '') => fire(b, sel, `new KeyboardEvent('keydown', {bubbles: true, cancelable: true, key: ${JSON.stringify(k)}, ${extra}})`);

/** Right-click `id`, open Color and choose `slot`, as a user does. */
async function choose(b, id, slot) {
  await fire(b, row(id), `new MouseEvent('contextmenu', {bubbles: true, cancelable: true, clientX: 300, clientY: 200})`);
  assert.equal((await b.evaluate('WC.menu()')).open, true, 'the menu opens');
  await fire(b, '.c-menu [aria-haspopup]', `new MouseEvent('click', {bubbles: true})`);
  assert.equal((await b.evaluate('WC.menu()')).sub, true, 'the Color submenu opens');
  await fire(b, `.c-menu [data-slot="${slot}"]`, `new MouseEvent('click', {bubbles: true})`);
  assert.equal((await b.evaluate('WC.menu()')).open, false, 'choosing closes the menu');
}

test('rows inherit their group colour and an own colour wins', async t => {
  const b = await open(t);
  assert.deepEqual(await inks(b), START);
});

test('right-click selects an unselected row, and the menu offers six colours with the current one checked', async t => {
  const b = await open(t);
  await fire(b, row('resp'), `new MouseEvent('contextmenu', {bubbles: true, cancelable: true, clientX: 300, clientY: 200})`);
  await fire(b, '.c-menu [aria-haspopup]', `new MouseEvent('click', {bubbles: true})`);
  assert.deepEqual(await b.evaluate('WC.menu()'), {open: true, sub: true, checked: ['pink'], items: ['default', 'blue', 'cyan', 'violet', 'pink', 'grey']});
  assert.equal(await b.evaluate(`document.querySelector(${JSON.stringify(row('resp'))}).getAttribute('aria-pressed')`), 'true');
  await key(b, '.c-menu:not([hidden]) [aria-checked]', 'Escape');
  await key(b, '.c-menu:not([hidden]) button', 'Escape');
  assert.equal((await b.evaluate('WC.menu()')).open, false, 'Escape closes the menu');
});

test('a colour applies to the whole selection and undo restores it', async t => {
  const b = await open(t);
  await select(b, 'clk'); await select(b, 'temp', true);
  const before = await stroke(b, 'clk');
  await choose(b, 'clk', 'blue');
  const now = await inks(b);
  assert.deepEqual([now.clk, now.temp], ['blue', 'blue']);
  assert.notEqual(await stroke(b, 'clk'), before);
  assert.equal(await stroke(b, 'clk'), await stroke(b, 'temp'));
  assert.deepEqual((await b.evaluate('WC.steps()')).past, ['Color 2 rows Blue']);
  await key(b, '#demo-frame', 'z', 'ctrlKey: true');
  assert.deepEqual(await inks(b), START);
  assert.equal(await stroke(b, 'clk'), before);
  await key(b, '#demo-frame', 'z', 'ctrlKey: true, shiftKey: true');
  assert.equal((await inks(b)).clk, 'blue');
});

test('a group colour flows down, Default clears, and a no-op records nothing', async t => {
  const b = await open(t);
  await choose(b, 'axi', 'violet');
  const now = await inks(b);
  assert.deepEqual([now.valid, now.data, now.resp], ['violet', 'violet', 'pink']);
  await choose(b, 'axi', 'violet');
  assert.equal((await b.evaluate('WC.steps()')).past.length, 1, 'applying the same colour records nothing');
  await choose(b, 'axi', 'default');
  assert.equal((await inks(b)).data, 'default');
});

test('X keeps its colour on a tinted row and every slot is legible in both themes', async t => {
  const b = await open(t);
  const undef = await stroke(b, 'err', 'c-x');
  await choose(b, 'err', 'pink');
  assert.equal(await stroke(b, 'err', 'c-x'), undef);
  assert.notEqual(await stroke(b, 'err'), undef);
  for (const mode of ['dark', 'light']) {
    await fire(b, `#page-theme [data-mode="${mode}"]`, `new MouseEvent('click', {bubbles: true})`);
    const worst = await b.evaluate(`(() => {
      const rgb = s => s.match(/[\\d.]+/g).slice(0, 3).map(Number);
      const lum = ([r, g, b]) => { const f = v => { v /= 255; return v <= .03928 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }; return .2126 * f(r) + .7152 * f(g) + .0722 * f(b); };
      const bg = lum(rgb(getComputedStyle(document.querySelector('.c-wave')).backgroundColor));
      return Math.min(...WC.slots.map(s => { const d = document.createElement('span'); d.className = 't-' + s; document.body.append(d);
        const l = lum(rgb(getComputedStyle(d).color)); d.remove(); return (Math.max(l, bg) + .05) / (Math.min(l, bg) + .05); }));
    })()`);
    assert.ok(worst >= 3, `${mode}: weakest slot ${worst.toFixed(2)}:1`);
  }
});
