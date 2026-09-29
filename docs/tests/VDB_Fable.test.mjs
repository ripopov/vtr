// node --test --test-concurrency=1 docs/tests/VDB_Fable.test.mjs
// The VDB Fable design page: the embedded recording and export are the real
// Verilator output of docs/vdb-fable/soc.sv, the page's layer engine agrees with
// an independent reading of that data (folding, state machine, register map,
// writes, assertion), the query catalogue answers with citations, the scenarios,
// tabs, cursor and plan figures work, and the page passes the design-system
// guardrails (lint, contrast, 360 px) through the shared test library.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {join} from 'node:path';
import {test} from 'node:test';
import {browser, root, lintPage} from './design-system-lib.mjs';

const page = join(root, 'docs/VDB_Fable.html');
const html = await readFile(page, 'utf8');
const DATA = JSON.parse(html.split('\n').find(l => l.startsWith('const DATA = ')).slice('const DATA = '.length).replace(/;\s*$/, ''));
const V = DATA.vdb, T = DATA.trace;
const valueAt = (ch, t) => { let v = null; for (const [ct, cv] of ch) { if (ct <= t) v = cv; else break; } return v; };
const sig = path => T.signals[T.vars[path]];

async function open(t, options) {
  const b = await browser();
  t.after(() => b.close());
  await b.open(page, options);
  await b.wait('window.ready === true');
  return b;
}

test('the embedded data is the Verilator export and recording of soc.sv', () => {
  assert.equal(V.format, 'vtr-rtl-vdb');
  assert.equal(V.version, 2);
  assert.match(V.producer, /^Verilator 5\.050/);
  assert.equal(V.top, 'top');
  assert.equal(V.instances.length, 10);
  assert.equal(Object.keys(V.symbols).length, 89);
  assert.equal(V.processes.length, 69);
  assert.deepEqual(V.sources.map(s => s.path), ['soc.sv']);
  assert.equal(V.elaboration.work_dir, '.');
  assert.ok(!/\/home\/|\/tmp\//.test(JSON.stringify(V)), 'no machine paths in the export');
  assert.equal(T.unit, 'ps');
  assert.equal(T.end, 400);
  assert.equal(Object.keys(T.signals).length, 32);
  assert.equal(Object.values(T.signals).reduce((n, c) => n + c.length, 0), 306);
  for (const ch of Object.values(T.signals)) for (let i = 1; i < ch.length; i++) assert.ok(ch[i][0] > ch[i - 1][0], 'changes in time order');
  // Every symbol the binding names resolves to a recorded var, and every wave row's var to a signal.
  for (const [sym, rec] of Object.entries(V.trace_binding.signals)) if (V.symbols[sym]) assert.ok(T.vars[rec] !== undefined, `${sym} recorded as ${rec}`);
  assert.deepEqual(DATA.enums, {dma_state_t: [['IDLE', 0], ['REQ', 1], ['BUSY', 2], ['DONE', 3]]});
  assert.match(DATA.source, /module dma \(/);
});

test('the export loses what the design declares, as the page says', () => {
  const state = V.symbols['top.u_dma.state'];
  assert.equal(state.type.text, 'logic[1:0]', 'the enum is typed as its base');
  assert.equal(T.enum_tables.state, 8, 'the recording keeps the enum table');
  const csrMux = V.processes.find(p => p.owner === 'top.u_csr' && p.targets.includes('top.u_csr.rdata'));
  assert.ok(JSON.stringify(csrMux.body).includes('"value":"4\'h4"'), 'parameters are folded into constants');
  const write = V.processes.find(p => p.owner === 'top.u_csr' && p.mode === 'seq');
  assert.ok(JSON.stringify(write.body).includes('"kind":"Unsupported"'), 'the struct cast is unsupported but keeps its reads');
});

test('the engine agrees with an independent reading of the data', {timeout: 60000}, async t => {
  const b = await open(t);
  assert.match(await b.evaluate('document.getElementById("selftest").textContent'), /selftest: all passed \(\d+ checks\)/);
  // Folding: definitions are module plus parameters; the eight-bit and sixteen-bit pipelines differ.
  const defs = await b.evaluate('VF.DEFS.map(d => [d.key, d.instances.length])');
  assert.deepEqual(defs, [['top', 1], ['csr', 1], ['dma', 1], ['arbiter', 1], ['pipe#(W=0x8)', 1], ['pipe#(W=0x10)', 1], ['stage#(W=0x8)', 2], ['stage#(W=0x10)', 2]]);
  // The state machine, brute force: transitions of the recorded state register.
  const ch = sig('TOP.top.u_dma.state').map(([t, v]) => [t, parseInt(v, 2)]);
  const names = ['IDLE', 'REQ', 'BUSY', 'DONE'];
  const seen = new Set(); for (let i = 1; i < ch.length; i++) if (ch[i][1] !== ch[i - 1][1]) seen.add(`${names[ch[i - 1][1]]}>${names[ch[i][1]]}`);
  const fsm = await b.evaluate('VF.Q.fsm("top.u_dma", 400)');
  assert.deepEqual(fsm.states, names);
  assert.deepEqual(fsm.transitions.map(x => `${x.from}>${x.to}`), ['IDLE>REQ', 'REQ>BUSY', 'BUSY>DONE', 'DONE>IDLE']);
  assert.deepEqual(fsm.transitions.map(x => x.guard), ['ctrl.start', 'gnt', 'last', 'always']);
  for (const x of fsm.transitions) assert.equal(x.taken, [...ch].filter((c, i) => i && ch[i - 1][1] === names.indexOf(x.from) && c[1] === names.indexOf(x.to)).length, `${x.from}>${x.to}`);
  assert.deepEqual([...seen].sort(), fsm.transitions.filter(x => x.taken).map(x => `${x.from}>${x.to}`).sort());
  assert.equal((await b.evaluate('VF.Q.fsm("top.u_dma", 125)')).state, 'DONE');
  assert.equal((await b.evaluate('VF.Q.fsm("top.u_dma", 45)')).state, 'REQ');
  // Register writes, brute force: wen high at a rising edge.
  const clk = sig('TOP.clk'), wen = sig('TOP.wen'), addr = sig('TOP.addr'), wdata = sig('TOP.wdata');
  const writes = clk.filter(([, v]) => v === '1').map(([t]) => t).filter(t => valueAt(wen, t - 1) === '1').map(t => ({t, addr: parseInt(valueAt(addr, t - 1), 2), data: parseInt(valueAt(wdata, t - 1), 2)}));
  assert.deepEqual(await b.evaluate('VF.WRITES'), writes);
  assert.deepEqual(writes.map(w => w.data), [0xc3, 0x81]);
  const reg = await b.evaluate('VF.Q.register("CTRL", 125)');
  assert.deepEqual(reg.fields, {start: 1, irq_en: 1, burst: 3});
  assert.equal(reg.writes[0].fields.burst, 3);
  assert.deepEqual((await b.evaluate('VF.REGISTERS[0].registers.map(r => [r.name, r.address, r.access])')), [['CTRL', 0, 'RW'], ['STATUS', 4, 'RO'], ['COUNT', 8, 'RO']]);
  // The irq at 125 ps comes from done and ctrl.irq_en; irq is high from 125 to 135.
  const irq = sig('TOP.irq');
  assert.deepEqual(irq.filter(([, v]) => v === '1').map(([t]) => t), [125]);
  assert.equal(valueAt(irq, 134), '1'); assert.equal(valueAt(irq, 135), '0');
  const explain = await b.evaluate('VF.Q.explain("top.irq", 125)');
  assert.equal(explain.value, '1');
  assert.deepEqual(explain.drivers.map(d => d.symbol).sort(), ['top.ctrl', 'top.done']);
  // The assertion: the first request waits for the CPU and fails ##[0:2]; the second is granted at once.
  assert.deepEqual((await b.evaluate('VF.evaluate(VF.ASSERTIONS[0])')).map(r => r.pass), [false, true]);
  // One clock domain, rooted through the aliases, with an active-low asynchronous reset over nine registers.
  const domains = await b.evaluate('VF.Q.domains()');
  assert.deepEqual(domains, [{clock: 'top.clk:PosEdge', resets: ['top.rst_n (active low, asynchronous)'], registers: 9}]);
  // Identities resolve both ways.
  const id = await b.evaluate('VF.Q.resolve("TOP.top.u_dma.state")');
  assert.equal(id.path, 'top.u_dma.state');
  assert.match(id.id, /^D\d+\.\d+$/);
  assert.deepEqual(b.exceptions, []);
});

test('the quoted numbers match the data and the measurement script', () => {
  assert.match(html, /809\.3 MB of JSON/);
  assert.match(html, /6,934 instances of 373 definitions; one definition has 3,056 instances/);
  assert.match(html, /processes 485 MB · symbols 155 MB · connections 103 MB · source index 38 MB · instances 27 MB/);
  assert.match(html, /zstd -3 of the file: 17\.5 MB/);
  assert.match(html, /13\.2 s wall, 5\.9 GiB peak RSS/);
  assert.match(html, /210,088 symbols keyed by path, 159 characters on average/);
  assert.ok(html.includes('bench/vdb_size.py'), 'the measurement script is named');
  assert.equal(html.match(/<article class="vf-stage"/g).length, 12, 'twelve stages');
  assert.equal(html.match(/<tr><td>\d+<\/td><td>/g).length, 12, 'twelve rows in the stage table');
  assert.equal((html.match(/<li class="vf-layer">/g) || []).length, 9, 'nine layers');
  assert.equal((html.match(/<ol class="vf-rules">[\s\S]*?<\/ol>/)[0].match(/<li>/g) || []).length, 10, 'ten decisions');
});

test('scenarios, tabs, cursor, clicks and figures', {timeout: 90000}, async t => {
  const b = await open(t);
  const text = sel => b.evaluate(`document.querySelector(${JSON.stringify(sel)}).textContent`);
  assert.match(await text('#vf-story'), /^Why did the IRQ fire/);
  assert.match(await text('#vf-now'), /state DONE · irq 1 · ctrl start=1 irq_en=1 burst=3/);
  assert.equal(await b.evaluate('document.querySelectorAll("#vf-view-idioms .vf-fsm .node").length'), 4);
  assert.equal(await b.evaluate('document.querySelector("#vf-view-idioms .vf-fsm .node.is-now text").textContent'), 'DONE');
  await b.click('#vf-prev');
  assert.equal(await b.evaluate('VF.S.cursor'), 115);
  assert.equal(await b.evaluate('document.querySelector("#vf-view-idioms .vf-fsm .node.is-now text").textContent'), 'BUSY');
  await b.click('[data-scenario="csr"]');
  assert.equal(await b.evaluate('VF.S.layer'), 'interfaces');
  assert.equal(await b.evaluate('VF.S.cursor'), 35);
  assert.match(await text('#vf-view-interfaces'), /CTRL.*0x0.*RW/s);
  assert.equal(await b.evaluate('document.querySelectorAll("#vf-view-interfaces [data-time]").length'), 2);
  await b.click('#vf-view-interfaces [data-time="205"]');
  assert.equal(await b.evaluate('VF.S.cursor'), 205);
  await b.click('[data-scenario="fold"]');
  assert.match(await text('#vf-view-structure'), /stage#\(W=0x10\).*2 instances/s);
  assert.match(await text('#vf-view-structure'), /openC9106,934210,088227,945809\.3 MB/);
  await b.click('[data-scenario="agent"]');
  assert.equal(await b.evaluate('document.querySelectorAll("#vf-view-knowledge .vf-turn--tool").length'), 6);
  assert.match(await text('#vf-view-knowledge'), /Expected\. At 35 ps software wrote CTRL = 0xc3 \(start=1, irq_en=1, burst=3\)/);
  assert.equal(await b.evaluate('VF.S.findings.length'), 1);
  assert.equal(await b.evaluate('document.querySelectorAll("#vf-view-knowledge tbody tr").length'), 1);
  // Source tab: the DMA's lines with values at the cursor; clicking a declaration selects it.
  await b.click('[data-layer="source"]');
  assert.ok(await b.evaluate('document.querySelector("#vf-view-source .row.is-current") !== null'));
  assert.match(await text('#vf-view-source .row.is-current'), /irq/);
  await b.click('#vf-view-source a[data-sym="top.u_dma.state"]').catch(() => {});
  await b.click('[data-layer="behaviour"]');
  assert.match(await text('#vf-view-behaviour'), /Static drivers of top\./);
  await b.click('[data-layer="presentation"]');
  assert.match(await text('#vf-view-presentation'), /winning rule/);
  await b.click('#vf-profile [data-profile="off"]');
  assert.equal(await b.evaluate('VF.S.profile'), false);
  // Plan figures are drawn by the engine.
  for (const n of [1, 2, 3, 5, 6, 7, 8, 9]) assert.ok(await b.evaluate(`document.getElementById('fig-${n}').innerHTML.length > 200`), `figure ${n}`);
  for (const n of [4, 11]) assert.ok(await b.evaluate(`document.getElementById('fig-${n}').height > 40`), `canvas ${n}`);
  assert.match(await b.evaluate('document.getElementById("fig-6").querySelector(".node.is-now text").textContent'), /DONE/);
  // Keyboard: arrows step clock edges on the focused waves.
  await b.evaluate('document.getElementById("vf-canvas").focus(); VF.setCursor(100)');
  for (const type of ['rawKeyDown', 'keyUp']) await b.send('Input.dispatchKeyEvent', {type, key: 'ArrowRight', windowsVirtualKeyCode: 39});
  assert.equal(await b.evaluate('VF.S.cursor'), 105);
  assert.deepEqual(b.exceptions, []);
});

for (const width of [1280, 360]) test(`layout, links and theme at ${width}px`, {timeout: 60000}, async t => {
  const b = await open(t, {width, height: 900});
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no page overflow');
  const broken = await b.evaluate(`[...document.querySelectorAll('a[href^="#"]')].filter(a => !document.getElementById(a.hash.slice(1))).map(a => a.hash)`);
  assert.deepEqual(broken, [], 'in-page links resolve');
  const unnamed = await b.evaluate(`[...document.querySelectorAll('button')].filter(e => !e.textContent.trim() && !e.getAttribute('aria-label') && !e.title).length`);
  assert.equal(unnamed, 0, 'buttons have names');
  assert.equal(await b.evaluate('document.querySelectorAll(".v-callout").length'), 0, 'no callout boxes');
  assert.equal(await b.evaluate('document.getElementById("demo").compareDocumentPosition(document.getElementById("problem")) & Node.DOCUMENT_POSITION_FOLLOWING'), 4, 'the explorer follows the hero');
  await b.click('#page-theme [data-mode="light"]');
  assert.equal(await b.evaluate('document.documentElement.dataset.theme'), 'light');
  assert.match(await b.evaluate('document.getElementById("selftest").textContent'), /all passed/);
  assert.deepEqual(b.exceptions, []);
});

test('the page keeps the design-system rules', () => {
  assert.deepEqual(lintPage(html), []);
});
