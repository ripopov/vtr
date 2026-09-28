// node --test docs/tests/undo-redo.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../undo-redo.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const CTRL = 2, META = 4, SHIFT = 8;
const state = b => b.evaluate('UR.state()');
async function open(width = 1280) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 1000, deviceScaleFactor: 1, mobile: false});
  await b.evaluate('UR.speed = 25; UR.render()');
  return b;
}
async function key(b, key, modifiers = 0) {
  const code = key.length === 1 ? `Key${key.toUpperCase()}` : key;
  const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : {Delete: 46, Escape: 27, Enter: 13}[key];
  const text = key.length === 1 && !(modifiers & (CTRL | META)) ? (modifiers & SHIFT ? key.toUpperCase() : key) : undefined;
  await b.send('Input.dispatchKeyEvent', {type: text ? 'keyDown' : 'rawKeyDown', key: modifiers & SHIFT && key.length === 1 ? key.toUpperCase() : key, code, windowsVirtualKeyCode: vk, modifiers, text});
  await b.send('Input.dispatchKeyEvent', {type: 'keyUp', key, code, windowsVirtualKeyCode: vk, modifiers});
}
const center = (b, selector) => b.evaluate(`(() => { const e = document.querySelector(${JSON.stringify(selector)});
  if (!e) throw new Error('missing ' + ${JSON.stringify(selector)}); const r = e.getBoundingClientRect(); return {x: r.x + r.width / 2, y: r.y + r.height / 2}; })()`);
async function mouse(b, type, p, extra = {}) {
  await b.send('Input.dispatchMouseEvent', {type, x: p.x, y: p.y, button: type === 'mouseMoved' ? 'none' : 'left', clickCount: 1, ...extra});
}
async function click(b, selector, extra = {}) {
  const p = await center(b, selector);
  await mouse(b, 'mouseMoved', p); await mouse(b, 'mousePressed', p, extra); await mouse(b, 'mouseReleased', p, extra);
}
async function drag(b, from, to, {release = true} = {}) {
  await mouse(b, 'mouseMoved', from); await mouse(b, 'mousePressed', from);
  for (let k = 1; k <= 6; k++) await mouse(b, 'mouseMoved', {x: from.x + (to.x - from.x) * k / 6, y: from.y + (to.y - from.y) * k / 6}, {buttons: 1});
  if (release) await mouse(b, 'mouseReleased', to);
}
async function focusDemo(b) {
  await b.evaluate(`document.getElementById('pv').scrollIntoView({block: 'start', behavior: 'instant'}); document.getElementById('pv').focus()`);
  await b.evaluate('document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))))');
}
const row = (panel, i) => `.upanel[data-panel="${panel}"] .urow[data-i="${i}"] .uname`;

for (const width of [1280, 390]) test(`page layout, self-test and accessibility at ${width}px`, {timeout: 30000}, async t => {
  const b = await open(width); t.after(() => b.close());
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /selftest: all passed/, selftest);
  assert.match(selftest, /[1-9]\d* evicted/, 'the self-test exercises eviction');
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links and citations resolve');
  const citations = await b.evaluate(`(() => { const refs = [...document.querySelectorAll('ol.refs li:not(.g)')].map(li => li.id);
    return [...document.querySelectorAll('sup.r a')].filter(a => a.textContent !== '[' + (refs.indexOf(a.hash.slice(1)) + 1) + ']').map(a => a.hash + ' ' + a.textContent); })()`);
  assert.deepEqual(citations, [], 'citation numbers match the reference list');
  const unnamed = await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`);
  assert.equal(unnamed, 0, 'buttons have names');
  assert.equal(await b.evaluate('document.querySelectorAll(".callout,.card,.badge").length'), 0, 'no callout boxes');
  assert.equal(await b.evaluate('document.querySelectorAll("[role=listbox] [role=option]").length'), 9, 'wave rows are options');
  assert.equal(await b.evaluate('document.querySelector(".uwave svg").clientWidth > 120'), true, 'waveforms are drawn');
  assert.deepEqual(b.exceptions, []);
});

test('chip and history text keep WCAG AA contrast in both themes', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.evaluate(`UR.setup('markers'); UR.render()`);
  for (const theme of ['light', 'dark']) {
    await b.evaluate(`document.documentElement.dataset.theme = '${theme}'`);
    const ratios = await b.evaluate(`(() => {
      const rgb = c => c.match(/[\\d.]+/g).slice(0, 3).map(Number);
      const lum = c => { const [r, g, b] = rgb(c).map(v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
      const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
      const cs = e => getComputedStyle(e);
      const chip = document.querySelector('.chip'), step = document.querySelector('#hist small'), panel = document.querySelector('.ur-hist');
      return {chip: ratio(cs(chip).color, cs(chip).backgroundColor), muted: ratio(cs(step).color, cs(panel).backgroundColor)};
    })()`);
    assert.ok(ratios.chip >= 4.5, `${theme} chip ${ratios.chip}`);
    assert.ok(ratios.muted >= 4.5, `${theme} history text ${ratios.muted}`);
  }
});

test('guided scenarios end in the states their text describes', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  const run = async name => { await b.click(`[data-guide="${name}"]`); await b.wait('!UR.state().busy && document.querySelector(\'[data-guide].on\')'); return state(b); };

  let s = await run('close');
  assert.deepEqual(s.order, [1, 2]); assert.equal(s.focused, 2);
  assert.deepEqual(s.panels[1].rows, ['[lsu]', '  valid:bin', '  ready:bin', '  addr:dec', '  wdata:dec'], 'rows and formats come back');
  assert.deepEqual(s.panels[1].sel, [3], 'selection is restored');
  assert.deepEqual([s.undo, s.redo], [[], ['Close Waves 2']]);
  assert.equal(s.notice, 'Undid Close Waves 2');

  s = await run('merge');
  assert.equal(s.panels[0].rows[3], 'count:hex');
  assert.deepEqual([s.undo, s.redo], [[], ['Format bin']], 'two presses were one step');

  s = await run('drag');
  assert.equal(s.panels[0].rows[0], 'count:hex', 'the row drag landed');
  assert.deepEqual(s.markers, ['1@95', '2@320', '3@410'], 'marker 2 moved; the cancelled drag of 3 left no trace');
  assert.deepEqual(s.undo, ['Move 1 row', 'Move marker 2']);
  assert.equal(s.open, false);

  s = await run('nav');
  assert.deepEqual(s.markers, []); assert.deepEqual([s.undo, s.redo], [[], ['Add marker 1']]);
  assert.equal(s.view.b - s.view.a, 320, 'the zoom survives undo');
  assert.equal(s.cursor, 320, 'the cursor survives undo');
  assert.equal(s.panels[1].rows[0], '[lsu]+', 'the fold survives undo');
  assert.deepEqual(s.panels[0].sel, [1, 2], 'the selection survives undo');

  s = await run('markers');
  assert.deepEqual(s.markers, ['1@95', '2@230', '3@410']);
  assert.deepEqual(s.redo, ['Clear 3 markers']);

  s = await run('branch');
  assert.deepEqual([s.undo, s.redo], [['Add marker 1'], []], 'a new edit ends the redo branch');
  assert.equal(s.panels[0].rows.length, 4);
  assert.deepEqual(b.exceptions, []);
});

test('keyboard: platform shortcuts, merging, removal and the history list', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.evaluate('UR.setClock(() => window.fakeNow ?? 0)');
  await focusDemo(b);
  await click(b, row(1, 3));
  await key(b, 't'); await key(b, 't');
  let s = await state(b);
  assert.equal(s.panels[0].rows[3], 'count:bin'); assert.deepEqual(s.undo, ['Format bin'], 'presses within a second merge');
  await b.evaluate('window.fakeNow = 5000');
  await key(b, 't');
  assert.deepEqual((await state(b)).undo, ['Format bin', 'Format hex'], 'a later press is its own step');
  await key(b, 'z', META);
  await key(b, 'z', CTRL);
  s = await state(b);
  assert.equal(s.panels[0].rows[3], 'count:hex'); assert.deepEqual(s.redo, ['Format bin', 'Format hex'], 'Cmd+Z and Ctrl+Z undo');
  await key(b, 'y', CTRL);
  await key(b, 'z', META | SHIFT);
  s = await state(b);
  assert.equal(s.panels[0].rows[3], 'count:hex'); assert.deepEqual([s.undo.length, s.redo.length], [2, 0], 'Ctrl+Y and Shift+Cmd+Z redo');
  await key(b, 'Delete');
  s = await state(b);
  assert.deepEqual(s.panels[0].rows, ['clk:bin', 'rst_n:bin', 'state:enum']); assert.equal(s.undo.at(-1), 'Remove 1 row');
  await key(b, 'z', META);
  s = await state(b);
  assert.equal(s.panels[0].rows[3], 'count:hex'); assert.deepEqual(s.panels[0].sel, [3], 'the restored row comes back selected');
  // navigation keys never reach the journal
  const before = JSON.stringify((await state(b)).undo.concat((await state(b)).redo));
  for (const k of ['=', '-', 'f']) await key(b, k);
  assert.equal(JSON.stringify((await state(b)).undo.concat((await state(b)).redo)), before);
  // the history list jumps
  await b.click('#hist [data-jump="0"]');
  s = await state(b);
  assert.deepEqual([s.undo.length, s.redo.length], [0, 3]);
  assert.equal(s.panels[0].rows[3], 'count:hex');
  await b.click('#hist [data-jump="2"]');
  assert.deepEqual((await state(b)).undo, ['Format bin', 'Format hex']);
  assert.deepEqual(b.exceptions, []);
});

test('pointer: a row drag and a marker drag are one step each; Escape cancels', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.evaluate(`UR.setup('markers'); UR.render()`);
  await focusDemo(b);
  const from = await center(b, row(1, 3)), to = await center(b, row(1, 0));
  await drag(b, from, {x: to.x, y: to.y - 9});
  let s = await state(b);
  assert.equal(s.panels[0].rows[0], 'count:hex'); assert.deepEqual(s.undo, ['Move 1 row']);
  // marker 2: moved and released
  const chip = await center(b, '.upanel[data-panel="1"] .chip[data-marker="2"]');
  await drag(b, chip, {x: chip.x + 90, y: chip.y});
  s = await state(b);
  assert.notEqual(s.markers.find(m => m.startsWith('2@')), '2@230'); assert.deepEqual(s.undo, ['Move 1 row', 'Move marker 2']);
  // marker 3: moved, then Escape before release
  const chip3 = await center(b, '.upanel[data-panel="1"] .chip[data-marker="3"]');
  await drag(b, chip3, {x: chip3.x + 60, y: chip3.y}, {release: false});
  assert.equal((await state(b)).gesture, true, 'the step is open during the drag');
  await key(b, 'Escape');
  await mouse(b, 'mouseReleased', {x: chip3.x + 60, y: chip3.y});
  s = await state(b);
  assert.ok(s.markers.includes('3@410'), 'Escape put marker 3 back'); assert.equal(s.undo.length, 2); assert.equal(s.open, false);
  // Shift-click removes a marker; undo restores it
  await click(b, '.upanel[data-panel="1"] .chip[data-marker="1"]', {modifiers: SHIFT});
  assert.equal((await state(b)).undo.at(-1), 'Remove marker 1');
  await key(b, 'z', META);
  assert.ok((await state(b)).markers.includes('1@95'));
  // closing with the × button, then undo
  await b.click('.upanel[data-panel="2"] [data-close]');
  assert.deepEqual((await state(b)).order, [1]);
  await key(b, 'z', CTRL);
  s = await state(b);
  assert.deepEqual(s.order, [1, 2]); assert.equal(s.focused, 2);
  assert.deepEqual(b.exceptions, []);
});

test('grouping: G and the name are one step; the name field keeps its own undo', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.evaluate(`UR.setup('one'); UR.render()`);
  await focusDemo(b);
  await click(b, row(1, 2));
  await click(b, row(1, 3), {modifiers: SHIFT});
  await key(b, 'g');
  await b.wait(`document.activeElement.id === 'rename'`);
  await b.send('Input.insertText', {text: 'fsm'});
  await key(b, 'z', META);     // text undo inside the field, not a cockpit undo
  let s = await state(b);
  assert.equal(s.renaming, true); assert.deepEqual(s.undo, ['Group rows'], 'the cockpit history is untouched');
  await b.evaluate(`document.getElementById('rename').value = 'fsm'`);
  await key(b, 'Enter');
  s = await state(b);
  assert.deepEqual(s.panels[0].rows, ['clk:bin', 'rst_n:bin', '[fsm]', '  state:enum', '  count:hex']);
  assert.deepEqual(s.undo, ['Group rows'], 'naming joined the group step');
  await key(b, 'z', META);
  s = await state(b);
  assert.deepEqual(s.panels[0].rows, ['clk:bin', 'rst_n:bin', 'state:enum', 'count:hex'], 'one undo removes the named group');
  assert.deepEqual(s.panels[0].sel, [2, 3]);
  assert.deepEqual(b.exceptions, []);
});
