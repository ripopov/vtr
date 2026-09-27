// node --test --test-concurrency=1 docs/tests/amba-chi-debug.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';

const html = await readFile(new URL('../amba-chi-debug.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};
const state = b => b.evaluate('CHIDEMO.state()');
async function open(width = 1366, height = 900) {
  const b = await browserTest(routes, {readyTimeout: 20000});
  await b.send('Emulation.setDeviceMetricsOverride', {width, height, deviceScaleFactor: 1, mobile: false});
  await b.evaluate('document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))))');
  return b;
}
const step = (b, box, name) => b.click(`#${box}-steps [data-step="${name}"]`);
async function mouse(b, type, p, extra = {}) { await b.send('Input.dispatchMouseEvent', {type, x: p.x, y: p.y, button: 'left', clickCount: 1, ...extra}); }
async function clickAt(b, p) { await mouse(b, 'mouseMoved', p, {button: 'none'}); await mouse(b, 'mousePressed', p); await mouse(b, 'mouseReleased', p); }
async function key(b, key, code, keyCode = 0) {
  for (const type of ['rawKeyDown', 'keyUp']) await b.send('Input.dispatchKeyEvent', {type, key, code, windowsVirtualKeyCode: keyCode, text: type === 'rawKeyDown' && key.length === 1 ? key : undefined});
}
const CANVASES = ['flow-cv', 'ring-map', 'ring-marey', 'line-cv', 'lat-cv', 'hang-strip', 'hang-graph'];
// A canvas is drawn when it has more than a few distinct colours.
const drawn = (b, id) => b.evaluate(`(() => { const c = document.querySelector('#${id} canvas'); const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; const s = new Set(); for (let i = 0; i < d.length; i += 400) s.add(d[i] << 16 | d[i + 1] << 8 | d[i + 2]); return s.size; })()`);
const corner = (b, id) => b.evaluate(`(() => { const c = document.querySelector('#${id} canvas'); const d = c.getContext('2d').getImageData(c.width - 3, c.height - 3, 1, 1).data; return '#' + [d[0], d[1], d[2]].map(v => v.toString(16).padStart(2, '0')).join(''); })()`);

for (const width of [1366, 390]) test(`page layout, self-test and accessibility at ${width}px`, {timeout: 60000}, async t => {
  const b = await open(width); t.after(() => b.close());
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /^selftest: all passed/, selftest);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links resolve');
  assert.equal(await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`), 0, 'buttons have names');
  for (const id of CANVASES) assert.ok(await drawn(b, id) > 8, `${id} is drawn`);
  assert.equal(await b.evaluate('document.querySelectorAll(".callout,.card,.badge").length'), 0, 'no callout boxes');
  assert.equal(await b.evaluate(`[...document.querySelectorAll('svg[role=img]')].every(s => s.querySelector('title') && s.querySelector('desc'))`), true, 'figures have a title and a description');
  assert.deepEqual(b.exceptions, []);
});

test('both themes: every mock redraws with the theme and keeps readable contrast', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  const seen = {};
  for (const theme of ['light', 'dark']) {
    await b.evaluate(`document.documentElement.dataset.theme = '${theme}'`);
    await b.evaluate('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))');
    assert.equal((await state(b)).theme, theme);
    const bg = await b.evaluate('CHIDEMO.C.bg');
    for (const id of CANVASES) {
      assert.equal(await corner(b, id), bg, `${id} is painted with the ${theme} background`);
      assert.ok(await drawn(b, id) > 8, `${id} is drawn in ${theme}`);
    }
    seen[theme] = bg;
    // WCAG ratios: text 4.5 (7 for body text), graphics 3 (1.4.11), captions on fills 4.5.
    const r = await b.evaluate(`(() => { const {C} = CHIDEMO;
      const lum = h => { const m = h.match(/[0-9a-f]{2}/gi).slice(0, 3).map(x => parseInt(x, 16) / 255).map(v => v <= .03928 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4); return .2126 * m[0] + .7152 * m[1] + .0722 * m[2]; };
      const cr = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + .05) / (y + .05); };
      const bad = [];
      const need = (name, a, b, min) => { const v = cr(a, b); if (v < min) bad.push(name + ' ' + v.toFixed(2)); };
      need('text', C.tx, C.bg, 7); need('muted text', C.mu, C.bg, 4.5); need('error', C.err, C.bg, 3);
      for (const [k, v] of Object.entries(C.ch)) need('channel ' + k, v, C.bg, 3);
      for (const [k, v] of Object.entries(C.stLine)) if (v) need('state label ' + k, v, C.bg, 3);
      for (const [k, v] of Object.entries(C.path)) need('class ' + k, v, C.bg, 3);
      for (const [k, v] of Object.entries(C.st)) if (v) need('text on state ' + k, C.onFill, v, 4.5);
      for (const [k, v] of Object.entries(C.agentFill)) need('text on request ' + k, C.onFill, v, 4.5);
      need('text on one holder', C.onFill, C.dir1, 4.5); need('text on two holders', C.onFill, C.dir2, 4.5);
      for (const v of C.data) need('data chip', v, C.band, 1.3);
      for (const [k, v] of Object.entries(C.cat)) need('category ' + k, v, C.bg, 1.25);
      need('caption', C.capTx, C.bg, 7); need('critical path', C.crit, C.bg, 1.9);
      const css = n => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
      need('page ink', css('--ink'), css('--bg'), 7); need('page muted', css('--muted'), css('--bg'), 4.5);
      need('mock text', css('--pv-tx'), css('--pv-p'), 7); need('mock muted', css('--pv-mu'), css('--pv-p'), 4.5);
      return bad; })()`);
    assert.deepEqual(r, [], `${theme} contrast`);
    // prose channel colours come from the same profile hues as the canvases
    assert.equal(await b.evaluate(`getComputedStyle(document.documentElement).getPropertyValue('--req').trim()`), await b.evaluate('CHIDEMO.C.ch.REQ'));
  }
  assert.notEqual(seen.light, seen.dark);
  // the page's button flips the theme
  await b.click('#theme');
  assert.equal((await state(b)).theme, 'light');
  assert.deepEqual(b.exceptions, []);
});

test('architecture: the analyses know no protocol, and survive renaming every name', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  const leaks = await b.evaluate(`CHIDEMO.CAUSAL.open.toString().match(/chi\\.|dj\\.|TX\\.|Comp[A-Z]|Snp|Read[A-Z]|hnf|cc[01]|RN-F|HN-F|received_by|blocked_by/g)`);
  assert.equal(leaks, null, 'volna-core logic names no CHI stream, key, opcode or relation');
  const r = await b.evaluate(`(() => { const {CAUSAL, PROFILE, TR} = CHIDEMO;
    const R = [['TX.chi.', 'sys.fab.'], ['TX.chi', 'sys.fab'], ['chi.', 'p.'], ['dj.', 'h.'], ['zj.', 'z.'], ['ring.', 'r.'], ['received_by', 'delivered_to'], ['blocked_by', 'queued_behind'], ['wait CompAck', 'await ack'], ['blocked', 'held']];
    const ren = s => typeof s !== 'string' ? s : R.reduce((a, [x, y]) => a.split(x).join(y), s);
    const renObj = o => Object.fromEntries(Object.entries(o).map(([k, v]) => [ren(k), typeof v === 'string' && /^TX\\./.test(v) ? ren(v) : v]));
    const deep = o => Array.isArray(o) ? o.map(deep) : o && typeof o === 'object' ? Object.fromEntries(Object.entries(o).map(([k, v]) => [ren(k), deep(v)])) : ren(o);
    const tr2 = {...TR, hier: TR.hier.map(h => ({...h, path: ren(h.path), attrs: renObj(h.attrs)})),
      recs: TR.recs.map(r => ({...r, stream: ren(r.stream), attrs: renObj(r.attrs), stages: r.stages.map(s => ({...s, name: ren(s.name)}))})),
      rels: TR.rels.map(x => ({...x, kind: ren(x.kind)}))};
    const A = CAUSAL.open(TR, PROFILE), B = CAUSAL.open(tr2, deep(PROFILE));
    const sig = M => JSON.stringify({
      cps: M.roots.map(r => M.criticalPath(r.id).segs.map(s => [s.a, s.b, M.CAT[s.cat]])),
      trees: M.roots.map(r => { const t = M.tree(r.id); return [t.recs.map(x => x.id), t.msgs.map(f => f.id)]; }),
      checks: M.check().map(c => [c.id, c.total, c.bad.map(x => x.rec.id)]),
      waits: M.waitChains().edges.map(e => [e.from.id, e.to.id]),
      hot: M.hotSubjects().map(h => [h.txns, h.moves, h.blocked, h.snoops]),
      hist: [...M.bySubject.keys()].map(a => { const h = M.history(a); return [Object.values(h.rn).map(v => v.map(e => [e.t, e.st, e.silent ?? null])), h.sf.map(e => e.t), h.faults.length]; }),
      classes: M.roots.map(M.classOf), transits: M.msgs.map(f => [f.path.join(), f.tInj]) });
    return {renamed: tr2.recs[0].stream, same: sig(A) === sig(B), n: B.roots.length}; })()`);
  assert.match(r.renamed, /^sys\.fab\./);
  assert.equal(r.n, 1468);
  assert.equal(r.same, true, 'every result is unchanged under renaming');
  // derived transits equal the model's ground truth
  assert.equal(await b.evaluate(`CHIDEMO.TR.flits.filter(f => { const g = CHIDEMO.M.msg.get(f.id); return !g || g.path.join() !== f.path.join() || g.tInj !== f.tInj || g.tEj !== f.tEj || g.dir !== f.dir || g.src !== f.src || g.tgt !== f.tgt; }).length`), 0);
  // the profile section prints the object the analyses run on
  const prof = await b.evaluate(`(() => { const t = document.getElementById('profile-json').textContent;
    const parsed = JSON.parse(t.replace(/\\/\\/[^\\n]*/g, ''));
    return {same: JSON.stringify(parsed) === JSON.stringify(CHIDEMO.PROFILE), swatches: document.querySelectorAll('#profile-json .swatch').length, width: Math.max(...t.split('\\n').map(l => l.length))}; })()`);
  assert.equal(prof.same, true, 'the printed profile parses back to the object the analyses run on');
  assert.ok(prof.width <= 110, `profile lines wrap (${prof.width} columns)`);
  assert.ok(prof.swatches >= 20, `${prof.swatches} hue swatches`);
  // the hierarchy tree is printed from the trace
  assert.match(await b.evaluate('document.getElementById("rec-tree").textContent'), /hf0p0 … pip\s+scope × 11/);
  assert.deepEqual(b.exceptions, []);
});

test('primer: the identifier figure explains each message from the trace', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  const pick = async i => { await b.click(`#ids tr[data-i="${i}"]`); return b.evaluate(`({ops: [...document.querySelectorAll('#ids tbody tr td:nth-child(3)')].map(td => td.textContent), key: [...document.querySelectorAll('#ids td.key')].map(td => td.textContent), match: document.querySelectorAll('#ids td.match').length, sent: document.querySelector('#ids .rc.sent')?.textContent ?? '', recv: document.querySelector('#ids .rc.recv')?.textContent ?? '', why: document.querySelector('#ids .ids-why').textContent})`); };
  let s = await pick(0);
  assert.deepEqual(s.ops, ['ReadNotSharedDirty', 'SnpNotSharedDirtyFwd', 'SnpRespFwded', 'CompData ×2', 'CompAck']);
  const data = await pick(3);
  assert.deepEqual(data.key, ['0x01', '0x1f'], 'DCT data is matched on TxnID = FwdTxnID and carries DBID = the snoop TxnID');
  assert.match(data.sent, /CC1 snoop/); assert.match(data.recv, /CC0 read/);
  assert.match(data.why, /Direct cache transfer/);
  const ack = await pick(4);
  assert.deepEqual(ack.key, ['0x1f']); assert.match(ack.recv, /HNF0 snoop/);
  assert.ok(ack.match >= 2, 'the DBID it echoes is tinted where it came from');
  await b.click('#ids-tools [data-flow="dmt"]');
  s = await pick(1);
  assert.match(s.why, /ReturnTxnID/); assert.match(s.recv, /SN read/);
  // keyboard moves between messages
  await b.evaluate(`document.querySelector('#ids tr[data-i="1"]').focus()`);
  await key(b, 'ArrowDown', 'ArrowDown', 40);
  assert.match(await b.evaluate(`document.querySelector('#ids tr.on').textContent`), /CompData/);
  assert.deepEqual(b.exceptions, []);
});

test('the model trace reproduces the hang and the analyses agree with it', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  const r = await b.evaluate(`(() => { const {M, HANGV} = CHIDEMO;
    const roots = M.roots.filter(r => r.status === 'ok');
    return {
      open: HANGV.W8.open.map(r => [r.stream, r.attrs['chi.addr']]),
      chain: HANGV.chains[0].nodes.map(r => r.stream),
      root: [HANGV.rootCause.stream, HANGV.rootCause.status, (M.outMsgs.get(HANGV.rootCause.id) ?? []).map(f => f.op)],
      failed: HANGV.CHECKS.filter(c => c.bad.length).map(c => [c.id, c.bad.length]),
      sameCycle: HANGV.sameCycle.map(f => f.op),
      unreceived: M.msgs.filter(f => !M.recvOf.has(f.id)).length,
      cpBad: roots.filter(r => { const c = M.criticalPath(r.id); return c.segs.reduce((a, s) => a + s.b - s.a, 0) !== r.end - r.begin; }).length,
      log: M.logs.map(l => l.text), answer: document.getElementById('agent-answer').textContent,
    }; })()`);
  assert.equal(r.open.length, 3);
  assert.ok(r.open.every(([, a]) => a === '0x80002000'), 'everything open is on the lock line');
  assert.deepEqual(r.chain, ['TX.chi.cc1.req', 'TX.chi.hnf0.task', 'TX.chi.hnf0.task', 'TX.chi.cc0.req']);
  assert.equal(r.root[1], 'ok');
  assert.equal(r.root[2].includes('CompAck'), false, 'the root never sent CompAck');
  assert.deepEqual(r.failed, [['compack', 1], ['home', 2]]);
  assert.deepEqual(r.sameCycle, ['SnpRespFwded']);
  assert.equal(r.unreceived, 0);
  assert.equal(r.cpBad, 0, 'critical paths add up to latency');
  assert.match(r.log[0], /^No instruction of core 1 commits for \d+ cycles, maybe get stuck$/);
  assert.match(r.answer, /blocked behind.*waits for CompAck.*closed at \d+ without sending CompAck/);
});

test('flow panel: guided flows draw the messages their text names', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  const msgs = () => b.evaluate(`CHIDEMO.FLOW.build().tree.msgs.map(f => f.op + ':' + f.src + '>' + f.tgt)`);
  await step(b, 'flow', 'dct');
  let m = await msgs();
  assert.ok(m.includes('SnpNotSharedDirtyFwd:hf0p0>cc1') && m.includes('CompData:cc1>cc0') && m.includes('CompAck:cc0>hf0p0'), m.join());
  assert.match(await b.evaluate('document.getElementById("flow-story").textContent'), /critical path: 25 cycles/);
  assert.equal(await b.evaluate('CHIDEMO.M.criticalPath(CHIDEMO.state().root).total'), 25);
  await step(b, 'flow', 'llc');
  m = await msgs(); assert.ok(m.some(x => /^CompData:hf\dp\d>cc\d$/.test(x)), 'LLC hit data comes from a home port');
  await step(b, 'flow', 'dmt');
  m = await msgs(); assert.ok(m.some(x => x.startsWith('ReadNoSnp:')) && m.some(x => x.startsWith('CompData:sn>')), 'DMT data comes from the SN');
  assert.ok(m.some(x => /^CompAck:cc\d>hf\dp1$/.test(x)), 'CompAck after DMT goes to port p1');
  await step(b, 'flow', 'wb');
  const wb = await b.evaluate(`CHIDEMO.FLOW.build().tree.recs.map(r => r.gen)`);
  assert.ok(wb.includes('writeback'), 'the LLC victim write-back is in the tree');
  await step(b, 'flow', 'hazard');
  const blocked = await b.evaluate(`CHIDEMO.FLOW.build().tree.recs.flatMap(r => r.stages).filter(s => s.name === 'blocked').map(s => s.e - s.b)`);
  assert.deepEqual(blocked, [70], 'the button says 70 cycles');
  assert.match(await b.evaluate('document.getElementById("flow-pv").closest("section").textContent'), /70 cycles in the PoS/);
  await step(b, 'flow', 'race');
  const race = await b.evaluate(`(() => { const r = CHIDEMO.M.byId.get(CHIDEMO.state().root); return [r.attrs['chi.opcode'], r.attrs['chi.resp'], CHIDEMO.M.recs.find(x => x.parent === r.id && x.attrs['dj.path'])?.attrs['dj.path']]; })()`);
  assert.deepEqual(race, ['WriteBackFull', 'I', 'stale write-back']);
  assert.ok(await b.evaluate('CHIDEMO.FLOW.build().ctxRecs.length') > 0, 'the competing ReadUnique is shown as context');
  await step(b, 'flow', 'ladder');
  assert.equal((await state(b)).orient, 'ladder');
  // clicking an arrow selects that flit
  await step(b, 'flow', 'dct');
  const p = await b.evaluate(`(() => { const a = CHIDEMO.FLOW.geo().arrows.find(a => !a.faint && a.f.op === 'CompAck'); const r = document.getElementById('flow-cv').getBoundingClientRect(); return {x: r.left + (a.x0 + a.x1) / 2, y: r.top + (a.y0 + a.y1) / 2, id: a.f.id}; })()`);
  await clickAt(b, p);
  assert.equal((await state(b)).sel, p.id);
  assert.match(await b.evaluate('document.getElementById("flow-side").textContent'), /CompAck/);
  // keys: O flips the layout, ] moves to the next request of the same L2
  const before = (await state(b)).root;
  await b.evaluate('document.getElementById("flow-pv").focus()');
  await key(b, 'o', 'KeyO', 79);
  assert.equal((await state(b)).orient, 'ladder');
  await key(b, ']', 'BracketRight', 221);
  const after = await state(b);
  assert.notEqual(after.root, before);
  assert.equal(await b.evaluate(`CHIDEMO.M.byId.get(${after.root}).stream`), 'TX.chi.cc0.req');
  assert.deepEqual(b.exceptions, []);
});

test('ring panel: utilization is derived exactly and the chart hit-tests flits', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await step(b, 'ring', 'burst');
  const s = await state(b);
  assert.deepEqual(s.ringWin, [3500, 5000]); assert.equal(s.ringCh, 'DAT');
  // prefix sums against a brute-force count over the model's own flits, for every DAT link
  const bad = await b.evaluate(`(() => { const {TR, RINGV} = CHIDEMO; const out = [];
    for (const d of ['cw', 'ccw']) for (let i = 0; i < 11; i++) {
      let n = 0; for (const f of TR.flits) if (f.ch === 'DAT' && f.dir === d) for (let k = 0; k < f.hops; k++) { const t = f.tInj + k; if (f.path[k] === i && t >= 3500 && t < 5000) n++; }
      if (Math.abs(RINGV.util('DAT', d, i, 3500, 5000) - n / 1500) > 1e-9) out.push(d + i);
    } return out; })()`);
  assert.deepEqual(bad, []);
  await step(b, 'ring', 'wait');
  const w = await b.evaluate(`(() => { const f = CHIDEMO.M.msg.get(CHIDEMO.state().sel); return {wait: f.tInj - f.tQ, story: document.getElementById('ring-story').textContent}; })()`);
  assert.ok(w.wait > 0); assert.match(w.story, /waited|entered the ring/);
  // a click on a flit's line selects it
  await step(b, 'ring', 'whole');
  await b.evaluate('CHIDEMO.RINGV.ax.show(420, 450); CHIDEMO.RINGV.redraw()');
  const p = await b.evaluate(`(() => { const {RINGV, M} = CHIDEMO; const ax = RINGV.ax, mg = RINGV.mg(); const r = document.getElementById('ring-marey').getBoundingClientRect();
    const f = M.msgs.find(f => f.tInj > 425 && f.tInj < 440 && f.hops >= 2 && Math.abs(f.path[0] - f.path[1]) === 1);
    return {x: r.left + (ax.x(f.tInj) + ax.x(f.tInj + 1)) / 2, y: r.top + (mg.y(f.path[0]) + mg.y(f.path[1])) / 2, id: f.id}; })()`);
  await clickAt(b, p);
  assert.equal((await state(b)).sel, p.id);
  await step(b, 'ring', 'follow');
  await b.wait('!CHIDEMO.RINGV.st.playing');
  assert.equal(await b.evaluate('CHIDEMO.RINGV.st.cursor === CHIDEMO.RINGV.st.win[1]'), true, 'playback reaches the end of the window');
  assert.deepEqual(b.exceptions, []);
});

test('line history: counter, race, silent upgrade and the lock line', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await step(b, 'line', 'counter');
  assert.equal((await state(b)).line, '0x80003040');
  const moves = await b.evaluate(`CHIDEMO.LINEV.rows()[0]`);
  assert.equal(moves.addr, '0x80003040'); assert.equal(moves.moves, 12);
  assert.match(await b.evaluate('document.querySelector("#line-steps [data-step=counter]").textContent'), /Ownership moves 12 times/);
  await step(b, 'line', 'race');
  const race = await b.evaluate(`(() => { const h = CHIDEMO.LINEV.st.hist; return {cc1: h.rn.cc1.map(e => e.st), cc0: h.rn.cc0.filter(e => e.t <= 5660).at(-1).st, empty: h.data.filter(d => !d.v).map(d => d.op), faults: h.faults.length}; })()`);
  assert.ok(race.cc1.includes('UD') && race.cc1.at(-1) === 'I');
  assert.equal(race.cc0, 'UD'); assert.deepEqual(race.empty, ['CopyBackWrData']); assert.equal(race.faults, 0);
  await step(b, 'line', 'silent');
  assert.equal(await b.evaluate(`['cc0', 'cc1'].some(a => CHIDEMO.LINEV.st.hist.rn[a].some(e => e.silent === 'UD'))`), true);
  await step(b, 'line', 'lock');
  const lock = await b.evaluate(`(() => { const r = CHIDEMO.M.byId.get(CHIDEMO.state().sel); return [CHIDEMO.state().line, r.status, r.stream]; })()`);
  assert.deepEqual(lock, ['0x80002000', 'open', 'TX.chi.hnf0.task']);
  // table rows switch the line
  await b.click('#line-table tr[data-a="0x80005040"]');
  assert.equal((await state(b)).line, '0x80005040');
  assert.deepEqual(b.exceptions, []);
});

test('latency panel: bands, the memory-bound burst and Little\'s law', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await step(b, 'lat', 'burst');
  const r = await b.evaluate(`(() => { const {LATV, M} = CHIDEMO; const ids = LATV.st.sel; const by = {};
    for (const id of ids) { const c = LATV.cp(M.byId.get(id)); for (const [k, v] of Object.entries(c.by)) by[k] = (by[k] ?? 0) + v; }
    const top = Object.entries(by).sort((a, b) => b[1] - a[1])[0][0];
    const kv = [...document.querySelectorAll('#lat-side .kv span')].map(s => s.textContent);
    const i = kv.indexOf('in flight, measured'), j = kv.indexOf('λ × W');
    return {n: ids.length, top, measured: parseFloat(kv[i + 1]), little: parseFloat(kv[j + 1])}; })()`);
  assert.ok(r.n > 100, `${r.n} selected`);
  assert.equal(r.top, 'mem', 'memory dominates the burst');
  assert.ok(Math.abs(r.measured - r.little) / r.measured < 0.1, `Little's law holds: ${r.measured} vs ${r.little}`);
  await step(b, 'lat', 'outliers');
  const o = await b.evaluate(`(() => { const {LATV, M} = CHIDEMO; let bl = 0, tot = 0; for (const id of LATV.st.sel) { const c = LATV.cp(M.byId.get(id)); bl += c.by.blocked ?? 0; tot += c.total; } return bl / tot; })()`);
  assert.ok(o > 0.4, `same-line wait is most of the outliers' time (${o})`);
  await step(b, 'lat', 'heat');
  assert.equal((await state(b)).latMode, 'heat');
  await step(b, 'lat', 'open');
  assert.equal(await b.evaluate('CHIDEMO.M.byId.get(CHIDEMO.state().sel).status'), 'open');
  // a click on a dot selects that request
  await step(b, 'lat', 'bands');
  const p = await b.evaluate(`(() => { const {LATV, M} = CHIDEMO; const L = LATV.lay(), r = M.roots.find(x => x.begin > 2000 && x.status === 'ok' && M.classOf(x) === 'DCT'); const c = document.getElementById('lat-cv').getBoundingClientRect(); return {x: c.left + LATV.ax.x(r.begin), y: c.top + L.y(r.end - r.begin), id: r.id}; })()`);
  await clickAt(b, p);
  const sel = await b.evaluate(`CHIDEMO.state().sel`);
  assert.equal(await b.evaluate(`CHIDEMO.M.classOf(CHIDEMO.M.byId.get(${sel}))`), 'DCT');
  assert.equal((await state(b)).root, sel, 'the Flow panel follows the selection');
  assert.deepEqual(b.exceptions, []);
});

test('hang panel steps select the chain and the explorer follows selections', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await step(b, 'hang', 'chain');
  assert.equal(await b.evaluate('CHIDEMO.M.byId.get(CHIDEMO.state().sel).stream'), 'TX.chi.cc1.req');
  await step(b, 'hang', 'root');
  const s = await state(b);
  assert.equal(s.sel, await b.evaluate('CHIDEMO.HANGV.rootCause.id'));
  assert.equal(s.gen, 'TX.chi.cc0.req/read', 'the explorer shows the selected record\'s generator');
  assert.match(await b.evaluate('document.getElementById("ex-raw").textContent'), /demo\.fault = "CompAck dropped"/);
  assert.equal(s.root, s.sel, 'the Flow panel opens the lost-CompAck request');
  assert.match(await b.evaluate('document.getElementById("hang-story").textContent'), /SnpRespFwded #\d+/);
  // explorer: choose a generator, then a record
  await b.click('#ex-tree .it[data-k="TX.chi.sn.task/read"]');
  await b.click('#ex-list .it[data-id]');
  assert.equal(await b.evaluate('CHIDEMO.M.byId.get(CHIDEMO.state().sel).stream'), 'TX.chi.sn.task');
  assert.deepEqual(b.exceptions, []);
});

test('references: numbered, external, and every in-text citation exists', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  const refs = await b.evaluate(`[...document.querySelectorAll('#refs ol.refs > li:not(.g)')].map(li => li.querySelector('a')?.href ?? '')`);
  assert.ok(refs.length >= 60, `${refs.length} references`);
  assert.ok(refs.filter(h => /^https?:\/\//.test(h)).length >= 55, 'references link out');
  const cited = await b.evaluate(`[...document.body.innerText.matchAll(/\\[(\\d+)(?:[–,] ?(\\d+))?[\\],]/g)].flatMap(m => [+m[1], m[2] ? +m[2] : null]).filter(Boolean)`);
  const max = Math.max(...cited.filter(n => n < 100));
  assert.ok(max <= refs.length, `citation [${max}] exists`);
  assert.match(refs[0], /documentation-service\.arm\.com/, 'the CHI specification is reference 1');
  assert.match(refs[61], /Charles_Ibry/, 'the Marey chart citation points at Ibry');
});
