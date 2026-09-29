// node --test --test-concurrency=1 docs/tests/volna-theme.test.mjs
// The Volna theme proposal: the measured floors hold for the tokens in
// docs/design-system/tokens/viewer.css, the demo draws each skin and vision
// mode, the theme sheet shows the token values, and the hierarchy browser
// searches, folds, pins, adds and reveals over the C910 data. The design-system
// guardrails cover the page's lint, contrast and 360px layout.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {join} from 'node:path';
import {test} from 'node:test';
import {browser, root} from './design-system-lib.mjs';

const page = join(root, 'docs/volna-theme.html');
const html = await readFile(page, 'utf8');
const DATA = JSON.parse(html.split('\n').find(l => l.startsWith('const DATA = ')).slice('const DATA = '.length).replace(/;\s*$/, ''));

/** Pixels of the demo canvas at CSS position (x, y), as [r, g, b]. */
const pixel = (b, id, x, y) => b.evaluate(`(() => { const c = document.getElementById(${JSON.stringify(id)}), s = c.width / c.clientWidth;
  return [...c.getContext('2d').getImageData(Math.round(${x} * s), Math.round(${y} * s), 1, 1).data.slice(0, 3)]; })()`);
/** Share of canvas pixels within `tol` of a token's colour. */
const share = (b, id, token, tol = 6) => b.evaluate(`(() => { const c = document.getElementById(${JSON.stringify(id)});
  const t = THEME.parse(getComputedStyle(document.getElementById('demo-frame')).getPropertyValue('--' + ${JSON.stringify(token)})).map(v => v * 255);
  const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let n = 0;
  for (let i = 0; i < d.length; i += 4) if (Math.abs(d[i] - t[0]) <= ${tol} && Math.abs(d[i + 1] - t[1]) <= ${tol} && Math.abs(d[i + 2] - t[2]) <= ${tol}) n++;
  return n / (d.length / 4); })()`);
const token = (b, theme, name) => b.evaluate(`THEME.parse(getComputedStyle(document.getElementById('probe-${theme}')).getPropertyValue('--${name}')).map(v => Math.round(v * 255))`);

test('the demo data is the landing trace window, in time order', () => {
  for (const [name, changes] of Object.entries(DATA.signals)) {
    assert.ok(changes.length > 0 && changes[0][0] === 717, `${name} starts at the window`);
    for (let i = 1; i < changes.length; i++) assert.ok(changes[i][0] > changes[i - 1][0], `${name} is in time order`);
  }
  assert.equal(DATA.signals['soc.cpu0.irq'].at(-1)[0], 757, 'the interrupt');
  assert.ok(DATA.signals['soc.l2.req_addr'].some(c => c[1] === 'x') && DATA.signals['soc.l2.resp_data'].some(c => c[1] === 'z'), 'X and Z on the L2 port');
  const aborted = DATA.pipeline.filter(t => t.status === 'aborted').map(t => t.id);
  assert.deepEqual(aborted, [239, 240], 'the flushed pair');
});

test('the proposed tokens meet the published floors in both themes', {timeout: 60000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page);
  await b.wait('window.ready === true');
  const a = await b.evaluate('THEME.audit()');
  for (const theme of ['dark', 'light']) {
    const v = a.volna[theme];
    for (const s of v.strokes) assert.ok(s.ratio >= 4.5, `${theme} ${s.role} stroke ${s.ratio.toFixed(2)}:1 < 4.5:1`);
    assert.ok(v.text >= 4.5, `${theme} stage text ${v.text.toFixed(2)}:1`);
    assert.ok(v.cvd >= 10, `${theme} neighbouring stages ${v.cvd.toFixed(1)} under simulated CVD`);
  }
  // The comparison is honest: today's One Dark is below at least one floor.
  assert.ok(a.today.stroke < 4.5 && a.today.text < 4.5 && a.today.cvd < 10, JSON.stringify(a.today));
  const shown = await b.evaluate(`['stat-stroke', 'stat-text', 'stat-cvd'].map(id => document.getElementById(id).textContent)`);
  const floor1 = x => (Math.floor(x * 10) / 10).toFixed(1);
  assert.deepEqual(shown, [floor1(Math.min(a.volna.dark.stroke, a.volna.light.stroke)) + ':1', floor1(Math.min(a.volna.dark.text, a.volna.light.text)) + ':1',
    floor1(Math.min(a.volna.dark.cvd, a.volna.light.cvd))]);
  assert.equal(await b.evaluate(`document.getElementById('stat-inline-text').textContent`), shown[1], 'the pipeline prose quotes the measured value');
  assert.deepEqual(b.exceptions, []);
});

test('the demo draws each theme and colour-vision mode', {timeout: 60000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page);
  await b.wait('window.ready === true');
  const canvas = async theme => (await token(b, theme, 'viewer-canvas')).slice(0, 3);
  // Volna Dark by default on a dark page: canvas right of the columns, below the header.
  assert.deepEqual(await pixel(b, 'demo-canvas', 1000, 60), await canvas('dark'));
  assert.ok(await share(b, 'demo-canvas', 'viewer-signal') > 0.002, 'signal strokes drawn');
  assert.ok(await share(b, 'demo-canvas', 'viewer-accent', 2) > 0.0003, 'cursor drawn');
  await b.click('#demo-skin [data-skin="volna-light"]');
  assert.equal(await b.evaluate('THEME.state().skin'), 'volna-light');
  assert.deepEqual(await pixel(b, 'demo-canvas', 1000, 60), await canvas('light'));
  // Volna Mixed: light chrome around the frame, the canvas drawn with the dark sheet.
  await b.click('#demo-skin [data-skin="volna-mixed"]');
  assert.equal(await b.evaluate(`document.getElementById('demo-frame').dataset.theme`), 'light');
  assert.deepEqual(await pixel(b, 'demo-canvas', 1000, 60), await canvas('dark'));
  await b.click('#demo-skin [data-skin="one-dark"]');
  assert.deepEqual(await pixel(b, 'demo-canvas', 1000, 60), await b.evaluate(`THEME.parse(getComputedStyle(document.getElementById('probe-dark')).getPropertyValue('--wave-bg')).map(v => Math.round(v * 255)).slice(0, 3)`));
  for (const vision of ['deutan', 'protan', 'tritan', 'grey', '']) {
    await b.click(`#demo-vision [data-vision="${vision}"]`);
    assert.equal(await b.evaluate(`document.getElementById('demo-canvas').style.filter`), vision ? `url("#cvd-${vision}")` : '');
  }
  // The page theme switch recolours the page; an explicit demo choice stays.
  await b.click('#page-theme [data-mode="light"]');
  assert.equal(await b.evaluate('document.documentElement.dataset.theme'), 'light');
  assert.equal(await b.evaluate('THEME.state().skin'), 'one-dark');
  await b.click('#values-vision [data-vision="grey"]');
  assert.equal(await b.evaluate(`document.getElementById('values-volna').style.filter`), 'url("#cvd-grey")');
  assert.deepEqual(b.exceptions, []);
});

test('Volna Mixed puts the dark data panels in a light window', {timeout: 60000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page);
  await b.wait('window.ready === true');
  const rgb = async (theme, name) => (await token(b, theme, name)).slice(0, 3);
  // Title bar 34px, sidebar 200px, then the wave panel: header 28px, marker lane 20px.
  const probe = () => Promise.all([pixel(b, 'mix-canvas', 100, 3), pixel(b, 'mix-canvas', 100, 300), pixel(b, 'mix-canvas', 1000, 34 + 60)]);
  assert.equal(await b.evaluate('THEME.mix.state().window'), 'mixed');
  assert.deepEqual(await probe(), [await rgb('light', 'viewer-bar'), await rgb('light', 'viewer-panel'), await rgb('dark', 'viewer-canvas')]);
  await b.click('#mix-skin [data-window="light"]');
  assert.deepEqual(await probe(), [await rgb('light', 'viewer-bar'), await rgb('light', 'viewer-panel'), await rgb('light', 'viewer-canvas')]);
  await b.click('#mix-skin [data-window="dark"]');
  assert.deepEqual(await probe(), [await rgb('dark', 'viewer-bar'), await rgb('dark', 'viewer-panel'), await rgb('dark', 'viewer-canvas')]);
  // The rules quote the measured seam, accents and the unchanged dark floor.
  const f = await b.evaluate('THEME.mix.facts()');
  assert.ok(f.seam >= 3 && f.cursor >= 4.5 && f.focus >= 4.5 && f.stroke >= 4.5, JSON.stringify(f));
  const floor1 = x => (Math.floor(x * 10) / 10).toFixed(1) + ':1';
  assert.deepEqual(await b.evaluate(`['stroke', 'seam', 'cursor', 'focus'].map(k => document.getElementById('mix-' + k).textContent)`),
    [f.stroke, f.seam, f.cursor, f.focus].map(floor1));
  assert.deepEqual(b.exceptions, []);
});

test('the theme sheet shows the token values', {timeout: 60000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page);
  await b.wait('window.ready === true');
  const rows = await b.evaluate(`[...document.querySelectorAll('#sheet-table tbody tr')].map(r => [...r.cells].map(c => c.textContent.trim()))`);
  assert.ok(rows.length >= 28, `${rows.length} rows`);
  const css = await readFile(join(root, 'docs/design-system/tokens/viewer.css'), 'utf8');
  const block = theme => css.slice(css.indexOf(theme === 'dark' ? ':root,[data-theme="dark"]{' : '[data-theme="light"]{')).split('}')[0];
  const hexOf = v => { const m = v.match(/^#([0-9a-f]{6})$/i); if (m) return '#' + m[1].toLowerCase();
    const r = v.match(/rgb\((\d+) (\d+) (\d+) \/ ([\d.]+)\)/); return '#' + [r[1], r[2], r[3]].map(x => (+x).toString(16).padStart(2, '0')).join('') + ' ' + Math.round(r[4] * 100) + '%'; };
  let checked = 0;
  for (const [role, dark, light] of rows) {
    const [, name] = html.match(new RegExp(`\\['${role.replace(/[’']/g, '.')}', '([\\w-]+)'`)) ?? [];
    if (!name) continue;
    for (const [theme, shown] of [['dark', dark], ['light', light]]) {
      const value = block(theme).match(new RegExp(`--${name}:([^;]+);`))[1];
      assert.equal(shown, hexOf(value), `${role} ${theme}`);
    }
    checked++;
  }
  assert.ok(checked >= 28, `${checked} sheet rows checked against viewer.css`);
});

const TRACE = JSON.parse(html.split('\n').find(l => l.startsWith('const TRACE = ')).slice('const TRACE = '.length).replace(/;\s*$/, ''));

test('the rendering lab holds the whole run and its clock stretches', () => {
  assert.deepEqual(TRACE.range, [0, 1685]);
  assert.deepEqual(TRACE.clocks['soc.dram.dram_clk'], [[0, 756, 3], [758, 1620, 2], [1661, 1685, 2]], 'boosted at the interrupt, gated at the end');
  for (const [name, changes] of Object.entries(TRACE.signals))
    for (let i = 1; i < changes.length; i++) assert.ok(changes[i][0] > changes[i - 1][0], `${name} in time order`);
  assert.ok(TRACE.signals['soc.l2.req_addr'].filter(c => c[1] === 'x').length > 50, 'the address is X while idle');
});

test('the rendering lab applies today\'s rules and the proposed ones', {timeout: 60000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page);
  await b.wait('window.ready === true');
  const lab = () => b.evaluate('THEME.lab.state()');
  const labels = async row => (await lab()).labels.filter(l => l[0] === row).map(l => l[1]);
  await b.click('#lab-view [data-view="whole"]');
  assert.deepEqual(await labels('clk'), ['500 MHz · 2 ns'], 'an aliased clock is a labelled band');
  assert.deepEqual(await labels('dram_clk'), ['333 MHz · 3 ns', '500 MHz · 2 ns'], 'the boost starts a new label');
  assert.ok((await labels('pc')).some(l => /^…[0-9a-f]{2,}$/.test(l)), 'numbers keep their last digits');
  // A column where X and real addresses mix keeps its activity fill and gets X rails.
  const g = await b.evaluate('THEME.lab.geometry()'), row = g.rows.find(r => r.row.name === 'req_addr');
  const cols = await b.evaluate(`(() => { const s = THEME.lab.sample(${JSON.stringify(TRACE.signals['soc.l2.req_addr'])}, 0, 1685, ${g.W});
    return [...s.kinds].map((m, c) => (m & 2) && (m & 1) && s.cnt[c] >= 1 ? c : -1).filter(c => c >= 0); })()`);
  assert.ok(cols.length > 5, `${cols.length} mixed columns`);
  const undef = (await token(b, 'dark', 'viewer-undef')).slice(0, 3);
  const c = cols[Math.floor(cols.length / 2)];
  assert.deepEqual(await pixel(b, 'lab-canvas', g.x0 + c, row.y + 5), undef, 'X rail');
  const [r, gr] = await pixel(b, 'lab-canvas', g.x0 + c, row.y + 7);
  assert.ok(gr > r, 'activity fill inside the rails');

  await b.click('#lab-rules [data-rules="today"]');
  assert.deepEqual(await labels('clk'), [], 'today a clock aliases into a band without a label');
  assert.ok((await labels('pc')).every(l => !l.startsWith('…')), 'today labels keep their first characters');
  assert.match(await b.evaluate(`THEME.lab.fitLabel('80000210', 48, true)`), /^…0*210$/);
  assert.match(await b.evaluate(`THEME.lab.fitLabel('80000210', 48, false)`), /^80+…$/);

  // The pointer finds the nearest edge within 6 px (proposed rules only), and drag pans.
  await b.click('#lab-rules [data-rules="proposed"]');
  await b.click('#lab-view [data-view="close"]');
  await b.evaluate(`document.getElementById('lab-canvas').scrollIntoView({block: 'center', behavior: 'instant'})`);
  const rect = await b.evaluate(`(() => { const r = document.getElementById('lab-canvas').getBoundingClientRect(); return {x: r.left, y: r.top}; })()`);
  const [vs, ve] = (await lab()).view, stall = g.rows.find(r => r.row.name === 'stall');
  const edge = TRACE.signals['soc.cpu0.stall'].find(ch => ch[0] > vs && ch[0] < ve)[0];
  const ex = g.x0 + (edge - vs) / (ve - vs) * g.W;
  await b.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: rect.x + ex + 4, y: rect.y + stall.y + 12});
  assert.deepEqual((await lab()).hover, {row: 'stall', edge});
  const pc = g.rows.find(r => r.row.name === 'pc');
  await b.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: rect.x + g.x0 + g.W / 2, y: rect.y + pc.y + 12});
  assert.match(String((await lab()).hover.value), /^[0-9a-f]+$/, 'a bus row reads the value under the pointer');
  for (const [type, dx] of [['mousePressed', 0], ['mouseMoved', -200], ['mouseReleased', -200]])
    await b.send('Input.dispatchMouseEvent', {type, x: rect.x + g.x0 + 400 + dx, y: rect.y + pc.y + 12, button: 'left', buttons: type === 'mouseReleased' ? 0 : 1, clickCount: 1});
  assert.ok((await lab()).view[0] > vs, 'dragging left moves later in time');
  assert.deepEqual(b.exceptions, []);
});

const HIER = JSON.parse(html.split('\n').find(l => l.startsWith('const HIER = ')).slice('const HIER = '.length).replace(/;\s*$/, ''));
const KEYS = {ArrowDown: 40, ArrowUp: 38, ArrowLeft: 37, ArrowRight: 39, Enter: 13, Tab: 9, Escape: 27};
async function key(b, k) {
  const code = KEYS[k];
  if (code) for (const type of ['rawKeyDown', 'keyUp']) await b.send('Input.dispatchKeyEvent', {type, key: k, code: k, windowsVirtualKeyCode: code});
  else { await b.send('Input.dispatchKeyEvent', {type: 'keyDown', key: k, text: k}); await b.send('Input.dispatchKeyEvent', {type: 'keyUp', key: k}); }
}
/** Mouse on the hierarchy canvas at CSS position (x, y). */
async function mouse(b, type, x, y, extra = {}) {
  const r = await b.evaluate(`(() => { const r = document.getElementById('hb-canvas').getBoundingClientRect(); return {x: r.left, y: r.top}; })()`);
  await b.send('Input.dispatchMouseEvent', {type, x: r.x + x, y: r.y + y, button: 'left', buttons: type === 'mouseReleased' || type === 'mouseMoved' && !extra.held ? 0 : 1, clickCount: extra.clickCount || 1});
}

test('the hierarchy data is the C910 trace, and the page quotes it', {timeout: 60000}, async t => {
  assert.deepEqual([HIER.vars, HIER.aliases, HIER.window, HIER.cursor], [204905, 137761, [250000, 251000], 250500]);
  const b = await browser();
  t.after(() => b.close());
  await b.open(page);
  await b.wait('window.ready === true');
  const f = await b.evaluate('THEME.hier.facts()');
  assert.deepEqual(f, {scopes: 6958, vars: 204905, aliases: 137761, inRuns: 2754, chains: 8, depth: 14, ifuDepth: 9, vfpu: [16, 5596], ifu: [2183, 6779], quiet: 804});
  const text = id => b.evaluate(`document.getElementById('${id}').textContent`);
  assert.equal(await text('hb-alias-share'), '67%');
  assert.equal(await text('hb-vfpu'), '16 of 5,596');
  assert.equal(await text('hb-runs'), '2,754 of 6,958');
  assert.equal(await text('hb-chains'), '8');
  // Every embedded IFU scope lists exactly the variables it counts.
  const ifu = await b.evaluate(`(() => { const {scopes, vars, byScope, ifu} = THEME.hier.data; let own = 0, bad = [];
    for (const s of scopes) { let i = s.id; while (i >= 0 && i !== ifu) i = scopes[i].parent; if (i !== ifu) continue;
      own += s.own; if ((byScope.get(s.id) || []).length !== s.own) bad.push(s.path); }
    return {own, vars: vars.length, bad, ifuVars: scopes[ifu].vars}; })()`);
  assert.deepEqual(ifu, {own: 16261, vars: 16261, bad: [], ifuVars: 16261});
  assert.deepEqual(b.exceptions, []);
});

test('the hierarchy browser ranks, merges, folds and pins', {timeout: 90000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page, {width: 1280});
  await b.wait('window.ready === true');
  const st = () => b.evaluate('THEME.hier.state()');
  const ifuPath = 'TOP.top.x_soc.x_cpu_sub_system_axi.x_rv_integration_platform.x_cpu_top.x_ct_top_0.x_ct_core.x_ct_ifu_top';
  let s = await st();
  assert.equal(s.selected, ifuPath, 'opens on the fetch unit');
  assert.equal(s.members.length, 1296);
  assert.ok(s.members.every(m => typeof m.value === 'string' && m.value.length), 'every variable has a cursor value');

  // Natural order and numbered runs.
  await b.evaluate(`(() => { const S = THEME.hier.data.scopes; for (const n of ['x_ct_rtu_top', 'x_ct_rtu_rob']) HBS.expanded.add(S.find(s => s.name === n).id); drawHier(); })()`);
  s = await st();
  const run = s.rows.find(r => r.kind === 'run' && r.stem === 'x_ct_rtu_rob_entry');
  assert.deepEqual([run.lo, run.hi, run.n], [0, 63, 64], 'the reorder buffer entries are one row');
  await b.evaluate(`(() => { HBS.runsOpen.add(HBS.rows.find(r => r.kind === 'run' && r.run.stem === 'x_ct_rtu_rob_entry').run.key); drawHier(); })()`);
  const entries = (await st()).rows.filter(r => r.kind === 'scope' && /\.x_ct_rtu_rob_entry\d+$/.test(r.path)).map(r => +r.path.match(/(\d+)$/)[1]);
  assert.deepEqual(entries, Array.from({length: 64}, (_, i) => i), 'opened, in numeric order');
  await b.click('#hb-rules [data-rules="today"]');
  assert.ok((await st()).rows.every(r => r.kind === 'scope'), 'today has no runs');
  const kids = (await st()).rows.filter(r => /\.x_ct_rtu_rob\.x_ct_rtu_rob_entry\d+$/.test(r.path)).map(r => r.path.split('.').pop());
  assert.deepEqual(kids.slice(0, 3), ['x_ct_rtu_rob_entry0', 'x_ct_rtu_rob_entry1', 'x_ct_rtu_rob_entry10'], 'today keeps file order');
  await b.click('#hb-rules [data-rules="proposed"]');

  // Search: ranked, highlighted, aliases merged, tree narrowed with counts.
  await b.click('#hb-try [data-q="ib inst0 data"]');
  s = await st();
  const top = s.members[0], leaf = top.path.slice(top.path.lastIndexOf('.') + 1);
  assert.equal(top.path, ifuPath + '.ifu_idu_ib_inst0_data', 'whole words in the leaf rank first');
  assert.ok(top.more >= 1, 'its alias in the fetch unit is merged');
  assert.deepEqual(top.pos.map(p => top.path[p]).join(''), 'ibinst0data', 'the matched characters');
  assert.ok(top.pos.every(p => p >= top.path.length - leaf.length));
  assert.ok(s.counts[ifuPath] >= s.members.length, 'the scope counts its results');
  assert.ok(s.rows.length < 20, `the tree narrows to ${s.rows.length} rows`);
  const sigs = await b.evaluate(`HBS.members.map(m => m.v.sig)`);
  assert.equal(new Set(sigs).size, sigs.length, 'one row per signal');
  await b.click('#hb-try [data-q="rob entry"]');
  s = await st();
  assert.ok(s.counts['TOP'] >= 64, 'matching scopes are counted');
  const G = await b.evaluate('THEME.hier.geometry(document.getElementById("hb-canvas").clientWidth)');
  const visible = s.rows.slice(s.scroll, s.scroll + G.treeRows);
  assert.ok(visible.some(r => r.kind === 'run' && r.stem === 'x_ct_rtu_rob_entry'), 'the tree scrolls to the first hit');
  assert.ok(s.sticky.length > 0 && s.sticky.length <= Math.floor(G.treeRows * 0.4), 'ancestors stay pinned, at most 40% of the pane');
  const firstUncovered = s.rows[s.scroll + s.sticky.length], under = firstUncovered.path || '';
  assert.ok(s.sticky.every(p => under.startsWith(p + '.') || p === 'x_ct_rtu_rob_entry' || s.rows.some(r => r.path === p)), 'the pinned rows are ancestors of the first row left uncovered');
  // Clearing gives the tree back.
  await b.click('#hb-try [data-q=""]');
  assert.equal((await st()).selected, ifuPath);
  // Today: a substring of the name in the selected scope, no ranking and no tree changes.
  await b.click('#hb-rules [data-rules="today"]');
  await b.evaluate(`THEME.hier.setQuery('inst0_data')`);
  s = await st();
  assert.ok(s.members.length > 0 && s.members.every(m => m.path.split('.').pop().includes('inst0_data') && m.path.startsWith(ifuPath + '.') && m.path.split('.').length === ifuPath.split('.').length + 1));
  assert.equal(s.counts, null);
  assert.deepEqual(b.exceptions, []);
});

test('the hierarchy browser adds by keyboard, double-click and drag, and reveals', {timeout: 90000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page, {width: 1280});
  await b.wait('window.ready === true');
  await b.evaluate(`document.getElementById('hb-canvas').scrollIntoView({block: 'center', behavior: 'instant'})`);
  const st = () => b.evaluate('THEME.hier.state()');
  const G = await b.evaluate('THEME.hier.geometry(document.getElementById("hb-canvas").clientWidth)');
  const ifuPath = (await st()).selected;
  // Keyboard: the tree first, Left to the parent, Tab to the variables, Enter adds.
  await b.evaluate(`document.getElementById('hb-canvas').focus()`);
  await key(b, 'ArrowLeft'); await key(b, 'ArrowLeft');
  assert.equal((await st()).selected, ifuPath.slice(0, ifuPath.lastIndexOf('.')), 'Left collapses, then goes to the parent');
  for (const next of ['x_ct_cp0_top', 'x_ct_idu_top', 'x_ct_ifu_top']) {
    await key(b, 'ArrowDown');
    assert.equal((await st()).selected, ifuPath.slice(0, ifuPath.lastIndexOf('.')) + '.' + next, 'Down walks the children in order');
  }
  await key(b, 'Tab'); await key(b, 'ArrowDown'); await key(b, 'Enter');
  let s = await st();
  assert.equal(s.focus, 'members');
  assert.deepEqual(s.waves, [s.members[1].path], 'Enter adds the selected variable');
  // Double-click adds; dragging onto Waves inserts where the line shows.
  const row = i => G.memTop + i * G.ROW + 12;
  await mouse(b, 'mousePressed', 60, row(3)); await mouse(b, 'mouseReleased', 60, row(3));
  await mouse(b, 'mousePressed', 60, row(3), {clickCount: 2}); await mouse(b, 'mouseReleased', 60, row(3), {clickCount: 2});
  s = await st();
  assert.deepEqual(s.waves, [s.members[1].path, s.members[3].path]);
  await mouse(b, 'mousePressed', 60, row(5));
  await mouse(b, 'mouseMoved', G.wx + 100, 40, {held: true});
  await mouse(b, 'mouseMoved', G.wx + 120, 34, {held: true});
  await mouse(b, 'mouseReleased', G.wx + 120, 34);
  s = await st();
  assert.deepEqual(s.waves, [s.members[5].path, s.members[1].path, s.members[3].path], 'dropped above the first row');
  // Typing in the tree goes to the search field; Escape clears it.
  await b.evaluate(`document.getElementById('hb-canvas').focus()`);
  await key(b, 'p');
  assert.equal(await b.evaluate(`document.activeElement.id`), 'hb-query');
  assert.equal((await st()).query, 'p');
  await key(b, 'Escape');
  assert.equal((await st()).query, '');
  // A wave row reveals its variable.
  const target = s.waves[0];
  await b.evaluate(`THEME.hier.setQuery(''); HBS.selected = 0; HBS.expanded = new Set([0]); drawHier()`);
  await mouse(b, 'mousePressed', G.wx + 40, 32 + 12); await mouse(b, 'mouseReleased', G.wx + 40, 32 + 12);
  s = await st();
  assert.equal(s.selected + '.' + target.split('.').pop(), target, 'the scope is selected');
  assert.equal(s.members[s.member].path, target, 'and the variable');
  const i = s.rows.findIndex(r => r.path === s.selected);
  assert.ok(i >= s.scroll && i < s.scroll + G.treeRows, 'scrolled into view');
  // The accent bar marks variables already in the wave panel.
  const accent = await b.evaluate(`THEME.parse(getComputedStyle(document.getElementById('hb-frame')).getPropertyValue('--viewer-accent')).slice(0, 3).map(v => Math.round(v * 255))`);
  assert.deepEqual(await pixel(b, 'hb-canvas', 0.5, G.memTop + (s.member - s.mscroll) * G.ROW + 12), accent);
  assert.deepEqual(b.exceptions, []);
});

test('the rendering lab draws an all-zero bus as a low line', {timeout: 60000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page);
  await b.wait('window.ready === true');
  const lab = () => b.evaluate('THEME.lab.state()');
  const labels = async row => (await lab()).labels.filter(l => l[0] === row).map(l => l[1]);
  await b.click('#lab-view [data-view="irq"]');
  assert.ok((await lab()).zeros > 0, 'zero segments drawn as low lines');
  assert.ok(!(await labels('mshr_used')).includes('0') && (await labels('mshr_used')).includes('1'), 'zero loses its label, nonzero keeps it');
  await b.click('#lab-rules [data-rules="today"]');
  assert.ok((await labels('mshr_used')).includes('0'), 'today zero is a labelled hexagon');
  assert.deepEqual(b.exceptions, []);
});
