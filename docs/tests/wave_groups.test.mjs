// node --test docs/tests/wave_groups.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';
const html = await readFile(new URL('../wave_groups.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};

const state = b => b.evaluate('GR.state()');
const point = (b, name, part = 'name') => b.evaluate(`GR.point(${JSON.stringify(name)}, ${JSON.stringify(part)})`);
const row = async (b, name) => (await state(b)).rows.find(r => r.name === name);
async function mouse(b, type, p, extra = {}) {
  await b.send('Input.dispatchMouseEvent', {type, x: p.x, y: p.y, button: 'left', clickCount: 1, ...extra});
}
async function click(b, p, extra = {}) {
  await mouse(b, 'mouseMoved', p, {button: 'none'});
  await mouse(b, 'mousePressed', p, extra);
  await mouse(b, 'mouseReleased', p, extra);
}
async function drag(b, from, to, steps = 8) {
  await mouse(b, 'mouseMoved', from, {button: 'none'});
  await mouse(b, 'mousePressed', from);
  for (let k = 1; k <= steps; k++)
    await mouse(b, 'mouseMoved', {x: from.x + (to.x - from.x) * k / steps, y: from.y + (to.y - from.y) * k / steps}, {buttons: 1});
  await mouse(b, 'mouseReleased', to);
}
async function key(b, key, code, modifiers = 0, keyCode = 0) {
  for (const type of ['rawKeyDown', 'keyUp'])
    await b.send('Input.dispatchKeyEvent', {type, key, code, modifiers, windowsVirtualKeyCode: keyCode, text: type === 'rawKeyDown' && key.length === 1 ? key : undefined});
}
const SHIFT = 8, ALT = 1;
async function guide(b, name) { await b.click(`[data-guide="${name}"]`); }
async function open(width = 1280) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height: 900, deviceScaleFactor: 1, mobile: false});
  await b.wait('window.ready === true');
  await b.evaluate('document.getElementById("pv").scrollIntoView({block: "center", behavior: "instant"})');
  await b.evaluate('document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))))');
  return b;
}

for (const width of [1280, 390]) test(`page layout, self-test and accessibility at ${width}px`, {timeout: 30000}, async t => {
  const b = await open(width); t.after(() => b.close());
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /selftest: all passed/, selftest);
  assert.doesNotMatch(selftest, /FAIL/);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links resolve');
  const unnamed = await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`);
  assert.equal(unnamed, 0, 'buttons have names');
  assert.equal(await b.evaluate('document.querySelectorAll(".callout,.card,.badge").length'), 0, 'no callout boxes');
  assert.equal(await b.evaluate('document.querySelectorAll("table").length <= 2'), true, 'at most two tables');
  assert.equal(await b.evaluate('document.getElementById("wbody").clientWidth > 200'), true, 'waves panel is visible');
  assert.equal(await b.evaluate('document.getElementById("dia-rows").childElementCount > 20'), true, 'diagram is drawn');
  const s = await state(b);
  assert.equal(s.valid, null);
  assert.deepEqual(s.visible.slice(0, 5), ['clk', 'rst_n', 'AXI master', 'Write', 'awvalid']);
  assert.equal(s.visible.includes('arvalid'), false, 'folded Read hides its rows');
  assert.match(await b.evaluate('document.getElementById("wsjson").textContent'), /"type": "group", "name": "Read", "collapsed": true|"name": "Read",\s*"collapsed": true/);
  assert.deepEqual(b.exceptions, []);
});

test('folded summary shows activity and the undefined cycle; figures agree', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  await guide(b, 'collapse');
  const w = await row(b, 'Write');
  assert.equal(w.collapsed, true); assert.equal(w.selected, true);
  const s = await state(b);
  assert.equal(s.visible.includes('wdata'), false);
  // The X colour sits inside the folded row at 510 ns; a quiet stretch is not red.
  const p = await point(b, 'Write', 'wave');
  const red = await b.evaluate(`GR.pixel(GR.timeX(508) - (document.getElementById('wbody').getBoundingClientRect().left), ${p.y} - document.getElementById('wbody').getBoundingClientRect().top + 5)`);
  assert.ok(red[0] > red[1] + 20, `undefined stretch is drawn in the X colour (${red})`);
  const vcell = await b.evaluate(`(() => { const s = GR.state(); return s.cursor; })()`);
  assert.equal(vcell, 510);
  // Figures: the blank heading has no green; the activity figure has green and red.
  const colours = await b.evaluate(`(() => { const out = {}; for (const id of ['fig-blank', 'fig-activity']) {
    const c = document.getElementById(id), d = c.getContext('2d').getImageData(96, 40, c.width - 104, 34).data; let green = 0, red = 0;
    for (let k = 0; k < d.length; k += 4) { if (d[k + 1] > 150 && d[k] < 190 && d[k + 2] < 150) green++; if (d[k] > 180 && d[k + 1] < 140) red++; }
    out[id] = {green, red}; } return out; })()`);
  assert.equal(colours['fig-blank'].green, 0);
  assert.ok(colours['fig-activity'].green > 100 && colours['fig-activity'].red > 5, JSON.stringify(colours));
  // Hover lists the members' values without unfolding.
  const q = await point(b, 'Write', 'wave');
  await mouse(b, 'mouseMoved', {x: await b.evaluate('GR.timeX(510)'), y: q.y}, {button: 'none'});
  const tip = await b.evaluate('document.querySelector(".tip").hidden ? null : document.querySelector(".tip").textContent');
  assert.match(tip ?? '', /wdata x/);
  assert.match(tip, /awvalid/);
  assert.deepEqual(b.exceptions, []);
});

test('chevron, keyboard folding and selection hand-off', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  // Click a member of Write, then fold Write by its chevron: the selection moves to the group.
  await click(b, await point(b, 'wdata'));
  assert.deepEqual((await state(b)).selected, ['wdata']);
  await click(b, await point(b, 'Write', 'chevron'));
  let s = await state(b);
  assert.equal((await row(b, 'Write')).collapsed, true);
  assert.deepEqual(s.selected, ['Write']);
  // → unfolds, → again goes to the first child, ← goes to the parent, ← folds.
  await key(b, 'ArrowRight', 'ArrowRight', 0, 39);
  assert.equal((await row(b, 'Write')).collapsed, false);
  await key(b, 'ArrowRight', 'ArrowRight', 0, 39);
  assert.deepEqual((await state(b)).selected, ['awvalid']);
  await key(b, 'ArrowLeft', 'ArrowLeft', 0, 37);
  assert.deepEqual((await state(b)).selected, ['Write']);
  await key(b, 'ArrowLeft', 'ArrowLeft', 0, 37);
  assert.equal((await row(b, 'Write')).collapsed, true);
  // Alt+click on AXI master's chevron folds it and every group inside.
  await click(b, await point(b, 'AXI master', 'chevron'), {modifiers: ALT});
  s = await state(b);
  assert.equal((await row(b, 'AXI master')).collapsed, true);
  await click(b, await point(b, 'AXI master', 'chevron'));
  assert.equal((await row(b, 'Read')).collapsed, true, 'inner groups stay folded after a plain unfold');
  // Enter toggles a group.
  await click(b, await point(b, 'Read'));
  await key(b, 'Enter', 'Enter', 0, 13);
  assert.equal((await row(b, 'Read')).collapsed, false);
  assert.deepEqual(b.exceptions, []);
});

test('group a selection with G, rename it, then ungroup', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  await click(b, await point(b, 'awvalid'));
  await click(b, await point(b, 'awaddr'), {modifiers: SHIFT});
  assert.deepEqual((await state(b)).selected, ['awvalid', 'awready', 'awaddr']);
  await key(b, 'g', 'KeyG', 0, 71);
  let s = await state(b);
  assert.equal(s.renaming, 'Group 1', 'G starts renaming the new group');
  assert.equal(await b.evaluate('document.activeElement.id'), 'rename');
  await b.send('Input.insertText', {text: 'AW channel'});
  await key(b, 'Enter', 'Enter', 0, 13);
  s = await state(b);
  assert.equal(s.renaming, null);
  const g = s.rows.findIndex(r => r.name === 'AW channel');
  assert.ok(g > 0 && s.rows[g].depth === 2 && s.rows[g + 1].name === 'awvalid' && s.rows[g + 1].depth === 3);
  assert.deepEqual(s.selected, ['AW channel']);
  // Double-click renames; Escape cancels.
  const p = await point(b, 'AW channel');
  await click(b, p);
  await mouse(b, 'mousePressed', p, {clickCount: 2}); await mouse(b, 'mouseReleased', p, {clickCount: 2});
  assert.equal((await state(b)).renaming, 'AW channel');
  await b.send('Input.insertText', {text: 'discarded'});
  await key(b, 'Escape', 'Escape', 0, 27);
  assert.ok((await state(b)).rows.some(r => r.name === 'AW channel'));
  // Shift+G ungroups and selects the former children.
  await key(b, 'G', 'KeyG', SHIFT, 71);
  s = await state(b);
  assert.equal(s.rows.some(r => r.name === 'AW channel'), false);
  assert.deepEqual(s.selected, ['awvalid', 'awready', 'awaddr']);
  assert.equal(s.rows.find(r => r.name === 'awvalid').depth, 2);
  assert.equal(s.valid, null);
  assert.deepEqual(b.exceptions, []);
});

test('drag with depth, drop into a folded group, never into itself', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  // irq is the last row; drag it up to the gap above itself? No: drag it right by one level
  // at the gap below Registers' header so it lands in Core.
  const irq = await point(b, 'irq');
  await drag(b, irq, {x: irq.left + irq.pad + irq.indent * 1 + 10, y: irq.top + 2});
  let s = await state(b);
  let r = s.rows.find(x => x.name === 'irq');
  assert.equal(r.depth, 1, 'the pointer x chose depth 1');
  const core = s.rows.findIndex(x => x.name === 'Core'), ii = s.rows.findIndex(x => x.name === 'irq');
  assert.ok(ii > core && s.rows.slice(core + 1, ii).every(x => x.depth >= 1), 'irq is now inside Core');
  // Drop onto the middle of the folded Read group: appended, group stays folded and selected.
  const read = await point(b, 'Read');
  await drag(b, await point(b, 'irq'), {x: read.x + 30, y: read.y});
  s = await state(b);
  const rd = s.rows.findIndex(x => x.name === 'Read'), end = s.rows.findIndex((x, k) => k > rd && x.depth <= s.rows[rd].depth);
  assert.equal(s.rows[end - 1].name, 'irq');
  assert.equal(s.rows[rd].collapsed, true);
  assert.deepEqual(s.selected, ['Read']);
  // Dragging AXI master into its own Write group does nothing.
  const before = JSON.stringify((await state(b)).rows.map(x => [x.name, x.depth]));
  const w = await point(b, 'wready');
  await drag(b, await point(b, 'AXI master'), {x: w.left + w.pad + w.indent * 2 + 10, y: w.top + w.row});
  assert.equal(JSON.stringify((await state(b)).rows.map(x => [x.name, x.depth])), before);
  assert.equal((await state(b)).valid, null);
  assert.deepEqual(b.exceptions, []);
});

test('guided scenarios, stepping, menu, clipboard and workspace text', {timeout: 40000}, async t => {
  const b = await open(); t.after(() => b.close());
  await guide(b, 'group');
  assert.ok((await state(b)).rows.some(r => r.name === 'AW channel' && r.group));
  await guide(b, 'into');
  assert.equal((await row(b, 'irq')).depth, 1);
  await guide(b, 'step');
  let s = await state(b);
  assert.deepEqual(s.selected, ['Read']);
  assert.ok(s.cursor > 100, 'Shift+→ stepped along the folded group');
  const c0 = s.cursor;
  await key(b, 'ArrowRight', 'ArrowRight', SHIFT, 39);
  assert.ok((await state(b)).cursor > c0);
  await key(b, 'ArrowLeft', 'ArrowLeft', SHIFT, 37);
  assert.equal((await state(b)).cursor, c0);
  await guide(b, 'scope');
  s = await state(b);
  assert.deepEqual(s.selected, ['u_dma']);
  assert.ok(s.rows.some(r => r.name === 'len' && r.depth === 1));
  assert.match(await b.evaluate('document.getElementById("wsjson").textContent'), /"u_dma"/);
  assert.equal(await b.evaluate('document.querySelectorAll("[data-template], [data-guide=profile]").length'), 0, 'VDB templates are postponed, not in the mock');
  await guide(b, 'ungroup');
  s = await state(b);
  assert.equal(s.rows.some(r => r.name === 'AXI master'), false);
  assert.equal((await row(b, 'Write')).depth, 0);
  // Right-click a group: the menu offers group commands; copy + paste duplicates the subtree.
  await b.evaluate('document.getElementById("pv").scrollIntoView({block: "center", behavior: "instant"})');
  const wr = await point(b, 'Write');
  await click(b, wr);
  await mouse(b, 'mouseMoved', wr, {button: 'none'});
  await mouse(b, 'mousePressed', wr, {button: 'right'}); await mouse(b, 'mouseReleased', wr, {button: 'right'});
  assert.equal((await state(b)).menu, true);
  const items = await b.evaluate('[...document.querySelectorAll(".menu button")].map(x => x.dataset.item + (x.disabled ? "-" : "+"))');
  assert.ok(items.includes('Ungroup+') && items.includes('Rename…+') && items.includes('Remove with contents+'), items.join());
  await b.click('.menu button[data-item="Copy"]');
  await key(b, 'v', 'KeyV', 4, 86);
  s = await state(b);
  assert.equal(s.rows.filter(r => r.name === 'Write').length, 2);
  assert.equal(s.valid, null);
  // Delete removes the pasted group with its contents.
  const n = s.rows.length;
  await key(b, 'Delete', 'Delete', 0, 46);
  s = await state(b);
  assert.equal(s.rows.filter(r => r.name === 'Write').length, 1);
  assert.ok(s.rows.length < n - 5);
  assert.deepEqual(b.exceptions, []);
});
