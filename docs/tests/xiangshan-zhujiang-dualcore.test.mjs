// node --test --test-concurrency=1 docs/tests/xiangshan-zhujiang-dualcore.test.mjs
// Owns a headless browser and loopback fixture; assertions are the review gate.
// The source test needs ext/XiangShan with its XSCache, XSCache/ZhuJiang and difftest
// submodules checked out; a missing checkout is a failure, not a skip.
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {existsSync} from 'node:fs';
import {readFile} from 'node:fs/promises';
import {test} from 'node:test';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';

const html = await readFile(new URL('../xiangshan-zhujiang-dualcore.html', import.meta.url), 'utf8');
const routes = {'/': {type: 'text/html', body: html}};
const XS = new URL('../../ext/XiangShan/', import.meta.url);

const state = b => b.evaluate('XZ.state()');
async function open(width = 1366, height = 800, media = []) {
  const b = await browserTest(routes);
  await b.send('Emulation.setDeviceMetricsOverride', {width, height, deviceScaleFactor: 1, mobile: false});
  if (media.length) await b.send('Emulation.setEmulatedMedia', {features: media});
  await b.wait('window.ready === true');
  await b.evaluate('document.getElementById("workspace").scrollIntoView({behavior: "instant"})');
  return b;
}
async function key(b, key, code, keyCode) {
  const text = key === 'Enter' ? '\r' : undefined;   // keyDown with text lets Enter activate native buttons
  for (const type of [text ? 'keyDown' : 'rawKeyDown', 'keyUp'])
    await b.send('Input.dispatchKeyEvent', {type, key, code, windowsVirtualKeyCode: keyCode, text: type === 'keyUp' ? undefined : text});
}
const KEYS = {Enter: ['Enter', 13], ArrowRight: ['ArrowRight', 39], ArrowLeft: ['ArrowLeft', 37], Escape: ['Escape', 27], Tab: ['Tab', 9]};
const press = (b, k) => key(b, k, KEYS[k][0], KEYS[k][1]);
const focused = b => b.evaluate('document.activeElement?.dataset?.node ?? document.activeElement?.id ?? null');

test('cited lines, pins and topology match the pinned XiangShan source', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  const {CITES, PIN, PARAMS, CFG} = await b.evaluate('({CITES: XZ.CITES, PIN: XZ.PIN, PARAMS: XZ.PARAMS, CFG: XZ.CFG})');
  const rev = dir => execFileSync('git', ['-C', new URL(dir, XS).pathname, 'rev-parse', 'HEAD'], {encoding: 'utf8'}).trim();
  assert.equal(rev('.'), PIN.xs, 'ext/XiangShan is at the pinned commit');
  assert.equal(rev('XSCache/'), PIN.xsc, 'XSCache is at the pinned commit');
  assert.equal(rev('XSCache/ZhuJiang/'), PIN.zj, 'ZhuJiang is at the pinned commit');
  assert.equal(rev('difftest/'), PIN.dt, 'difftest is at the pinned commit');
  const files = new Map();
  const lines = async f => { if (!files.has(f)) files.set(f, (await readFile(new URL(f, XS), 'utf8')).split('\n')); return files.get(f); };
  const bad = [];
  for (const [k, c] of Object.entries(CITES)) {
    const src = await lines(c.f);
    assert.ok(c.l[0] >= 1 && c.l[1] >= c.l[0] && c.l[1] <= src.length, `${k}: line range inside ${c.f}`);
    if (!src.slice(c.l[0] - 1, c.l[1]).join('\n').includes(c.x)) bad.push(`${k} ${c.f}:${c.l.join('-')} lacks ${JSON.stringify(c.x)}`);
  }
  assert.deepEqual(bad, [], 'every citation quotes text found at its lines');
  // The whole cited range is pinned by hash, so any edit inside it fails, not only one that removes the quote.
  const changed = [];
  for (const [k, c] of Object.entries(CITES)) {
    const digest = createHash('sha256').update((await lines(c.f)).slice(c.l[0] - 1, c.l[1]).join('\n') + '\n').digest('hex');
    if (digest !== c.h) changed.push(`${k} ${c.f}:${c.l.join('-')}`);
  }
  assert.deepEqual(changed, [], 'cited line ranges are unchanged (sha256)');
  // Nested pins must be the gitlinks recorded by the parent repositories, not just matching checkouts.
  const gitlink = (parent, child) => execFileSync('git', ['-C', new URL(parent, XS).pathname, 'ls-tree', 'HEAD', child], {encoding: 'utf8'}).split(/\s+/)[2];
  assert.equal(gitlink('.', 'XSCache'), PIN.xsc, 'XiangShan records the XSCache pin');
  assert.equal(gitlink('.', 'difftest'), PIN.dt, 'XiangShan records the difftest pin');
  assert.equal(gitlink('XSCache/', 'ZhuJiang'), PIN.zj, 'XSCache records the ZhuJiang pin');
  assert.equal(execFileSync('git', ['-C', new URL('../../', import.meta.url).pathname, 'ls-tree', 'HEAD', 'ext/XiangShan'], {encoding: 'utf8'}).split(/\s+/)[2], PIN.xs, 'this repository records the XiangShan pin');
  // Source links open the cited file in the checkout.
  const hrefs = await b.evaluate(`[...new Set([...document.querySelectorAll('a.srcpath')].map(a => a.getAttribute('href')))]`);
  assert.ok(hrefs.length >= 100);
  assert.deepEqual(hrefs.filter(h => !/^\.\.\/ext\/XiangShan\/[^#]+#L\d+$/.test(h) || !existsSync(new URL('../' + h.split('#')[0], import.meta.url))), [], 'source links resolve to files');
  // Self-contained: no network fetches of any kind.
  assert.doesNotMatch(html, /<(?:script|link|img|iframe)[^>]+(?:src|href)=["']?(?:https?:)?\/\/|@import|url\(\s*["']?https?:|fetch\(|XMLHttpRequest|WebSocket/);
  assert.ok(Object.keys(CITES).length >= 100, 'citations cover the diagram');

  // The page's ring is a literal copy of dualCoreNodeParams: compare it with the Scala.
  const topo = (await lines('src/main/scala/top/ZhuJiangNoCTopology.scala')).join('\n');
  const body = topo.slice(topo.indexOf('private def dualCoreNodeParams'));
  const scala = [...body.slice(0, body.indexOf('\n  )\n')).matchAll(/NodeParam\(\s*nodeType = NodeType\.(\w+)(?:, bankId = (\d+), hfpId = (\d+))?([^\n]*(?:\n\s+axiDevParams[^\n]*)?)/g)]
    .map(m => ({t: m[1], ...(m[2] !== undefined ? {bank: +m[2], hfp: +m[3]} : {}), defaultHni: /defaultHni = true/.test(m[4]) || undefined}));
  assert.deepEqual(scala.map(p => [p.t, p.bank, p.hfp]), PARAMS.map(p => [p.t, p.bank, p.hfp]), 'ring order, banks and ports');
  assert.deepEqual(scala.map(p => !!p.defaultHni), PARAMS.map(p => !!p.defaultHni), 'default HNI flag');
  assert.match(topo, /nodeAidBits = 3,/);
  // Capacity inputs quoted by the page.
  const cfg = (await lines('src/main/scala/top/Configs.scala')).join('\n');
  assert.match(cfg, /class DefaultConfig\(n: Int = 1\) extends Config\(\n[^\n]*\n\s+\+\+ ZhuJiangConfig\("32MB", ways = 16\)/);
  const zjp = (await lines('XSCache/ZhuJiang/src/main/scala/zhujiang/ZJParameters.scala')).join('\n');
  for (const re of [/hnxBankOff: Int = 12,/, /clusterCacheSizeInB: Int = 2 \* 1024 \* 1024,/, /snoopFilterWays: Int = 16,/, /hnxOutstanding: Int = 64 \* 4,/, /hnxDirSRAMBank: Int = 2,/,
    /private lazy val bank\s+= nodeParams.filter\(_.hfpId == 0\).count\(_.nodeType == NodeType.HF\)/, /llcSizeInB = cacheSizeInB \/ bank,/, /nrDSBank = hnxDirSRAMBank \* 2,/])
    assert.match(zjp, re);
  assert.equal(CFG.cacheSizeInB, 32 * 2 ** 20);
  assert.equal(CFG.llcPerBank, 16 * 2 ** 20);
  assert.equal(CFG.sfSets, 4096);
  assert.deepEqual(b.exceptions, []);
});

// Geometry probe run inside the page for the current level: rendered text sizes, fit and overlap.
const PROBE = `(() => {
  const s = document.getElementById('svg'), k = s.getScreenCTM().a, vb = s.viewBox.baseVal;
  const texts = [...s.querySelectorAll('text')].filter(t => t.textContent.trim());
  const rect = e => e.getBoundingClientRect();
  const hit = (a, c, m = 1) => a.left < c.right - m && c.left < a.right - m && a.top < c.bottom - m && c.top < a.bottom - m;
  const overlaps = [];
  // Labels on one line keep a 6 px gap; stacked lines may not overlap by more than 1 px.
  const clash = (a, c) => Math.abs(a.top - c.top) < 2 ? a.left < c.right + 6 && c.left < a.right + 6 : hit(a, c);
  for (let i = 0; i < texts.length; i++) for (let j = i + 1; j < texts.length; j++) if (clash(rect(texts[i]), rect(texts[j]))) overlaps.push(texts[i].textContent + ' | ' + texts[j].textContent);
  const boxes = [...s.querySelectorAll('#g-nodes [data-node]')].map(g => [g, g.querySelector('.box')]);
  const unfit = boxes.flatMap(([g, bx]) => { const r = bx.getBBox(); return [...g.querySelectorAll('text')].filter(t => { const q = t.getBBox(); return q.x < r.x + 2 || q.x + q.width > r.x + r.width - 2 || q.y < r.y || q.y + q.height > r.y + r.height; }).map(t => g.dataset.node + ': ' + t.textContent); });
  const onBoxes = [...s.querySelectorAll('#g-edges text, #g-frames text')].flatMap(t => boxes.filter(([, bx]) => hit(rect(t), rect(bx))).map(([g]) => t.textContent + ' on ' + g.dataset.node));
  const outside = boxes.filter(([, bx]) => { const r = bx.getBBox(); return r.x < 0 || r.y < 0 || r.x + r.width > vb.width || r.y + r.height > vb.height; }).map(([g]) => g.dataset.node);
  const px = texts.map(t => [parseFloat(getComputedStyle(t).fontSize) * k, t.textContent]).sort((a, c) => a[0] - c[0])[0];
  return {minPx: px[0], minText: px[1], overlaps, unfit, onBoxes, outside};
})()`;
const LEVELS = [['sys'], ['core'], ['noc'], ['hnf', 0], ['hnf', 1], ['mem']];

for (const [width, height] of [[1920, 1080], [1440, 900], [1366, 768], [1024, 768], [390, 844]]) test(`layout, readability and self-test at ${width}×${height}`, {timeout: 60000}, async t => {
  const b = await open(width, height); t.after(() => b.close());
  const selftest = await b.evaluate('document.getElementById("selftest").textContent');
  assert.match(selftest, /^selftest: all passed/, selftest);
  assert.doesNotMatch(selftest, /FAIL/);
  assert.equal(await b.evaluate('document.documentElement.scrollWidth <= innerWidth'), true, 'no horizontal page overflow');
  const geo = await b.evaluate(`(() => { const s = document.getElementById('svg').getBoundingClientRect(), d = document.querySelector('.side').getBoundingClientRect(), v = document.getElementById('view'); return {sw: s.width, sh: s.height, dl: d.left, dt: d.top, sr: s.right, sb: s.bottom, scroll: v.scrollWidth > v.clientWidth}; })()`);
  if (width >= 1320) assert.ok(geo.dl >= geo.sr, 'side panel beside the diagram');
  else assert.ok(geo.dt >= geo.sb, 'side panel below the diagram');
  assert.equal(geo.scroll, width < 960, 'only narrow screens scroll the diagram, inside its own region');
  for (const [level, bank] of LEVELS) {
    await b.evaluate(`XZ.setLevel(${JSON.stringify(level)}); ${bank !== undefined ? `XZ.setBank(${bank});` : ''} document.getElementById('workspace').scrollIntoView({behavior: 'instant'})`);
    const tag = `${level}${bank ?? ''}`;
    const r = await b.evaluate(PROBE);
    assert.ok(r.minPx >= 11.5, `${tag}: smallest diagram text "${r.minText}" is ${r.minPx.toFixed(2)} px`);
    assert.deepEqual(r.unfit, [], `${tag}: block labels fit inside their blocks`);
    assert.deepEqual(r.overlaps, [], `${tag}: no two diagram labels overlap`);
    assert.deepEqual(r.onBoxes, [], `${tag}: connection and frame labels stay off blocks`);
    assert.deepEqual(r.outside, [], `${tag}: blocks inside the view box`);
    if (width >= 1024) {
      const v = await b.evaluate(`(() => { const s = document.getElementById('svg').getBoundingClientRect(), h = document.querySelector('header').getBoundingClientRect(); return {top: s.top, bottom: s.bottom, head: h.bottom, vh: innerHeight}; })()`);
      assert.ok(v.top >= v.head && v.bottom <= v.vh, `${tag}: whole diagram visible below the header ${JSON.stringify(v)}`);
    }
  }
  // Ordinary page text is readable too.
  const html = await b.evaluate(`(() => { let min = [99, '']; const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    while (w.nextNode()) { const n = w.currentNode, e = n.parentElement; if (!n.textContent.trim() || e.closest('svg, #selftest') || !e.getClientRects().length) continue;
      const f = parseFloat(getComputedStyle(e).fontSize); if (f < min[0]) min = [f, n.textContent.trim().slice(0, 40)]; } return min; })()`);
  assert.ok(html[0] >= 11.5, `smallest page text "${html[1]}" is ${html[0]} px`);
  assert.deepEqual(b.exceptions, []);
});

// Linux CI renders with DejaVu Sans, which is wider than the macOS system font. Verdana is wider still,
// so passing with it (or with DejaVu where Verdana is absent) keeps the layout independent of the platform font.
test('layout keeps its margins with a wide fallback font', {timeout: 60000}, async t => {
  const wide = html.replace('</style>', 'svg text{font-family:Verdana,"DejaVu Sans",sans-serif !important}</style>');
  const b = await browserTest({'/': {type: 'text/html', body: wide}}); t.after(() => b.close());
  await b.send('Emulation.setDeviceMetricsOverride', {width: 1366, height: 768, deviceScaleFactor: 1, mobile: false});
  for (const [level, bank] of LEVELS) {
    await b.evaluate(`XZ.setLevel(${JSON.stringify(level)}); ${bank !== undefined ? `XZ.setBank(${bank});` : ''}`);
    const r = await b.evaluate(PROBE), tag = `${level}${bank ?? ''}`;
    assert.deepEqual(r.unfit, [], `${tag}: labels fit`); assert.deepEqual(r.overlaps, [], `${tag}: no overlaps`); assert.deepEqual(r.onBoxes, [], `${tag}: labels off blocks`);
  }
  assert.deepEqual(b.exceptions, []);
});

test('accessibility tree, names, focus order and contrast in both themes', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  // The skip link is the first Tab stop (before the header link), is visible when focused and jumps to the diagram.
  await b.evaluate('scrollTo(0, 0); document.querySelector(".brand").focus()');
  for (const type of ['rawKeyDown', 'keyUp']) await b.send('Input.dispatchKeyEvent', {type, key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9, modifiers: 8});
  assert.equal(await b.evaluate('document.activeElement.textContent'), 'Skip to the interactive diagram');
  assert.equal(await b.evaluate('[...document.querySelectorAll("a, button, [tabindex]")].find(e => e.tabIndex >= 0) === document.activeElement'), true, 'first focusable element');
  assert.equal(await b.evaluate('getComputedStyle(document.activeElement).top'), '10px');
  await press(b, 'Enter');
  assert.equal(await b.evaluate('location.hash'), '#diagram');
  const ax = async () => (await b.send('Accessibility.getFullAXTree')).nodes.filter(n => !n.ignored);
  const prop = (n, k) => n.properties?.find(p => p.name === k)?.value?.value;
  let tree = await ax();
  const tabs = tree.filter(n => n.role?.value === 'tab');
  assert.deepEqual(tabs.map(n => n.name.value), ['1 · System', '2 · XS core', '3 · NoC ring', '4 · HNF bank', '5 · Memory path'], 'level tabs are named tabs');
  assert.deepEqual(tabs.map(n => prop(n, 'selected') === true), [true, false, false, false, false]);
  assert.ok(tree.some(n => n.role?.value === 'tablist' && n.name?.value === 'Diagram level'));
  assert.ok(tree.some(n => n.role?.value === 'tabpanel'));
  assert.ok(tree.some(n => n.role?.value === 'link' && n.name?.value === 'Skip to the interactive diagram'));
  for (const name of ['(a) L1 miss that hits in the other core', '(b) LLC hit', '(c) LLC miss to memory', 'Previous step', 'Next step', 'Play', 'Exit', 'Toggle colour theme'])
    assert.ok(tree.some(n => n.role?.value === 'button' && n.name?.value === name), `button "${name}"`);
  for (const [level, bank] of LEVELS) {
    await b.evaluate(`XZ.setLevel(${JSON.stringify(level)}); ${bank !== undefined ? `XZ.setBank(${bank});` : ''}`);
    const labels = await b.evaluate(`[...document.querySelectorAll('#g-nodes [data-node]')].map(g => g.getAttribute('aria-label'))`);
    tree = await ax();
    const buttons = new Set(tree.filter(n => n.role?.value === 'button').map(n => n.name?.value));
    assert.deepEqual(labels.filter(l => !buttons.has(l)), [], `${level}${bank ?? ''}: every block is a named button in the accessibility tree`);
    assert.ok(tree.some(n => n.role?.value === 'group' && n.name?.value.length > 20), `${level}: the diagram is a named group`);
  }
  // aria-pressed reaches the accessibility tree.
  await b.evaluate('XZ.setLevel("noc")');
  await b.click('[data-node="noc.sn"]');
  tree = await ax();
  const sn = tree.find(n => n.role?.value === 'button' && n.name?.value.startsWith('0x38 · S'));
  assert.equal(prop(sn, 'pressed'), 'true', 'selected block is exposed as pressed');
  // Tab moves block to block; focus is drawn in the focus colour.
  await b.evaluate('document.querySelector("[data-node=\'noc.hf0p0\']").focus()');
  await press(b, 'Tab');
  assert.equal(await focused(b), 'noc.cc0');
  const ring = await b.evaluate('(() => { const r = getComputedStyle(document.activeElement.querySelector(".ring")), p = document.body.appendChild(document.createElement("i")); p.style.color = "var(--focus)"; const c = getComputedStyle(p).color; p.remove(); return [r.visibility, r.stroke, c, parseFloat(r.strokeWidth)]; })()');
  assert.equal(ring[0], 'visible');
  assert.equal(ring[1], ring[2], 'focus ring uses the theme focus colour');
  assert.ok(ring[3] >= 3, 'focus ring is at least 3 px wide');
  // Colour contrast (WCAG AA 4.5:1) of every diagram label against what it is drawn on, in both themes.
  for (const theme of ['light', 'dark']) {
    const worst = await b.evaluate(`(() => { document.documentElement.dataset.theme = '${theme}';
      const rgb = c => c.match(/[\\d.]+/g).slice(0, 3).map(Number);
      const lum = c => { const [r, g, v] = rgb(c).map(x => { x /= 255; return x <= .03928 ? x / 12.92 : ((x + .055) / 1.055) ** 2.4; }); return .2126 * r + .7152 * g + .0722 * v; };
      const cr = (a, c) => { const x = lum(a), y = lum(c); return (Math.max(x, y) + .05) / (Math.min(x, y) + .05); };
      const paper = getComputedStyle(document.querySelector('.panel')).backgroundColor; let worst = [99, ''];
      for (const lv of ['sys', 'core', 'noc', 'hnf', 'mem']) { XZ.setLevel(lv);
        for (const t of document.querySelectorAll('#svg text')) { const g = t.closest('[data-node]');
          const bg = !g ? paper : t.style.fontSize ? getComputedStyle(t.previousElementSibling.tagName === 'rect' ? t.previousElementSibling : g.querySelector('.box')).fill : getComputedStyle(g.querySelector('.box')).fill;
          const c = cr(getComputedStyle(t).fill, bg); if (c < worst[0]) worst = [c, lv + ': ' + t.textContent]; } }
      return worst; })()`);
    assert.ok(worst[0] >= 4.5, `${theme}: lowest label contrast ${worst[0].toFixed(2)} for ${worst[1]}`);
  }
  assert.deepEqual(b.exceptions, []);
});

test('every block on every level highlights exactly its connections and explains them with sources', {timeout: 120000}, async t => {
  const b = await open(); t.after(() => b.close());
  let total = 0;
  for (const [level, bank] of LEVELS) {
    await b.evaluate(`XZ.setLevel(${JSON.stringify(level)}); ${bank !== undefined ? `XZ.setBank(${bank});` : ''}`);
    const ids = await b.evaluate(`[...document.querySelectorAll('#g-nodes [data-node]')].map(g => g.dataset.node)`);
    for (const id of ids) {
      await b.click(`[data-node="${id}"]`);
      const r = await b.evaluate(`(() => { const id = ${JSON.stringify(id)};
        const want = [...document.querySelectorAll('#g-edges [data-edge]')].filter(g => g.dataset.a === id || g.dataset.b === id).map(g => g.dataset.edge).sort();
        return {sel: XZ.state().sel, want, hl: [...document.querySelectorAll('#g-edges .hl')].map(g => g.dataset.edge).sort(),
          conns: [...document.querySelectorAll('#detail [data-conn]')].map(li => li.dataset.conn).sort(),
          text: document.getElementById('detail-body').textContent.length, links: document.querySelectorAll('#detail a.srcpath').length,
          audit: !!document.querySelector('[data-audit-node="' + id + '"]'), auditEdges: want.every(e => document.querySelector('[data-audit-edge="' + e + '"]')) }; })()`);
      assert.equal(r.sel, id);
      assert.deepEqual(r.hl, r.want, `${id}: highlighted connections`);
      assert.deepEqual(r.conns, r.want, `${id}: every highlighted connection is explained`);
      assert.ok(r.text > 150, `${id}: explanation`); assert.ok(r.links >= 1, `${id}: source lines`);
      assert.ok(r.audit && r.auditEdges, `${id}: listed in the audit`);
      total++;
    }
  }
  assert.ok(total >= 80, `${total} blocks checked`);
  assert.deepEqual(b.exceptions, []);
});

test('NoC level: every stop in ring order, two HNF instances with distinct ports, links and capacities', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.evaluate('XZ.setLevel("noc")');
  const stops = await b.evaluate(`[...document.querySelectorAll('#g-nodes [data-stop]')].sort((a, c) => a.dataset.stop - c.dataset.stop).map(g => g.querySelector('.t1').textContent)`);
  assert.deepEqual(stops, ['0x00 · HF', '0x08 · CC', '0x10 · HF', '0x18 · CC', '0x20 · RI', '0x28 · HI', '0x30 · HF', '0x38 · S', '0x40 · HF', '0x48 · M', '0x50 · P']);
  const count = type => stops.filter(s => s.endsWith(' ' + type)).length;
  assert.deepEqual([count('HF'), count('CC'), count('RI'), count('HI'), count('S'), count('M'), count('P')], [4, 2, 1, 1, 1, 1, 1]);
  const s = await state(b);
  const links = s.edges.filter(e => /^noc\.link\d+$/.test(e));
  assert.equal(links.length, 11, 'one link between each pair of neighbouring stops, including the wrap-around');
  const ends = await b.evaluate(`[...document.querySelectorAll('[data-edge^="noc.link"]')].map(g => [g.dataset.a, g.dataset.b])`);
  const order = ['hf0p0', 'cc0', 'hf1p0', 'cc1', 'ri', 'hi', 'hf1p1', 'sn', 'hf0p1', 'mn', 'pip'];
  assert.deepEqual(ends, order.map((id, i) => [`noc.${id}`, `noc.${order[(i + 1) % 11]}`]));
  assert.equal(await b.evaluate('document.querySelectorAll("[data-edge^=\'noc.link\'] path.lane").length'), 22, 'two lanes (directions) per link');
  const banks = s.nodes.filter(n => /^noc\.bank\d$/.test(n));
  assert.deepEqual(banks, ['noc.bank0', 'noc.bank1'], 'four HF stops belong to exactly two HNF instances');
  const lans = await b.evaluate(`[...document.querySelectorAll('[data-edge^="noc.lan."]')].map(g => [g.dataset.a, g.dataset.b])`);
  assert.deepEqual(lans.sort(), [['noc.bank0', 'noc.hf0p0'], ['noc.bank0', 'noc.hf0p1'], ['noc.bank1', 'noc.hf1p0'], ['noc.bank1', 'noc.hf1p1']]);
  assert.equal(await b.evaluate('XZ.CFG.banks'), 2);
  // Unused RNI is marked, not hidden.
  assert.equal(await b.evaluate('document.querySelector("[data-node=\'noc.ri\']").classList.contains("unverified")'), true);
  // HNF level shows the capacity split for the chosen bank.
  await b.evaluate('XZ.setLevel("hnf")');
  const txt = await b.evaluate('document.getElementById("g-nodes").textContent');
  assert.match(txt, /LLC 32 MiB = 2 banks × 16 MiB/);
  assert.match(txt, /bank 0: PA\[12\]=0 · 16 MiB/); assert.match(txt, /16384 sets × 16 ways × 64 B/);
  assert.match(txt, /bank 1: PA\[12\]=1 · 16 MiB/);
  assert.match(txt, /p0 · 0x00/); assert.match(txt, /p1 · 0x40/);
  await b.click('[data-bank="1"]');
  const txt1 = await b.evaluate('document.getElementById("g-nodes").textContent');
  assert.match(txt1, /p0 · 0x10/); assert.match(txt1, /p1 · 0x30/);
  for (const id of ['hnf.llc', 'hnf.sf', 'hnf.data', 'hnf.be']) assert.ok(s !== null && (await state(b)).nodes.includes(id), `${id} drawn`);
  // Memory level states the boundary and that the SN is only a bridge.
  await b.evaluate('XZ.setLevel("mem")');
  assert.match(await b.evaluate('document.getElementById("hint").textContent'), /ZhuJiang ends at m_axi_mem_0.*not a DDR controller/);
  const mem = (await state(b)).nodes;
  for (const id of ['mem.sn', 'mem.axib', 'mem.port', 'mem.xbar', 'mem.simxbar', 'mem.ram', 'mem.axmem', 'mem.dram']) assert.ok(mem.includes(id), id);
  assert.equal(await b.evaluate('document.querySelector("[data-edge=\'mem.e.sx-axm\']").classList.contains("opt")'), true, 'DRAMsim3 path drawn as optional');
  assert.deepEqual(b.exceptions, []);
});

test('selecting blocks highlights connections and explains them, by mouse and keyboard', {timeout: 30000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.evaluate('XZ.setLevel("noc")');
  // Mouse: select the bank 0 p0 port.
  await b.click('[data-node="noc.hf0p0"]');
  let s = await state(b);
  assert.equal(s.sel, 'noc.hf0p0');
  assert.equal(await b.evaluate('document.querySelector("[data-node=\'noc.hf0p0\']").getAttribute("aria-pressed")'), 'true');
  const hl = await b.evaluate('[...document.querySelectorAll("#g-edges .hl")].map(g => g.dataset.edge).sort()');
  assert.deepEqual(hl, ['noc.lan.hf0p0', 'noc.link0', 'noc.link10']);
  const friends = await b.evaluate('[...document.querySelectorAll("#g-nodes .friend")].map(g => g.dataset.node).sort()');
  assert.deepEqual(friends, ['noc.cc0', 'noc.cc1'], 'egress friends of port 0x00');
  const detail = await b.evaluate('document.getElementById("detail").textContent');
  assert.match(detail, /Stop 0: 0x00 HF — HNF bank 0 port 0/);
  assert.match(detail, /egress friends \(targets sent from this port\): 0x08 CC, 0x18 CC/);
  assert.match(detail, /ext\/XiangShan\/XSCache\/ZhuJiang\/src\/main\/scala\/zhujiang\/ZJParameters\.scala:135–159/);
  assert.equal(await b.evaluate('document.getElementById("detail").getAttribute("aria-live")'), 'polite');
  // Keyboard: arrows follow ring order, Enter selects, Escape clears.
  await b.evaluate('document.querySelector("[data-node=\'noc.pip\']").focus()');
  await press(b, 'ArrowRight');
  assert.equal(await focused(b), 'noc.hf0p0', 'ring order wraps from stop 10 to stop 0');
  await press(b, 'ArrowRight');
  assert.equal(await focused(b), 'noc.cc0');
  const ringVisible = await b.evaluate('getComputedStyle(document.activeElement.querySelector(".ring")).visibility');
  assert.equal(ringVisible, 'visible', 'keyboard focus is visible');
  await press(b, 'Enter');
  s = await state(b);
  assert.equal(s.sel, 'noc.cc0');
  assert.deepEqual(await b.evaluate('[...document.querySelectorAll("#g-nodes .friend")].map(g => g.dataset.node).sort()'), ['noc.hf0p0', 'noc.hf1p0'], 'request friends of CC0');
  await press(b, 'Escape');
  assert.equal((await state(b)).sel, null);
  assert.equal(await b.evaluate('document.getElementById("svg").classList.contains("has-sel")'), false);
  // Level tabs: arrow keys move between levels and focus follows.
  await b.evaluate('document.getElementById("tab-noc").focus()');
  await press(b, 'ArrowRight');
  assert.equal((await state(b)).level, 'hnf');
  assert.equal(await focused(b), 'tab-hnf');
  assert.equal(await b.evaluate('document.getElementById("bankopt").getClientRects().length > 0'), true, 'bank selector shown on the HNF level');
  await press(b, 'ArrowRight');
  assert.equal((await state(b)).level, 'mem');
  assert.equal(await b.evaluate('document.getElementById("bankopt").getClientRects().length'), 0, 'bank selector hidden on other levels');
  await b.click('[data-node="mem.axib"]');
  assert.match(await b.evaluate('document.getElementById("detail").textContent'), /TgtID.*ReturnNID/);
  await b.click('#tab-sys');
  await b.click('[data-node="sys.sn"]');
  assert.match(await b.evaluate('document.getElementById("detail").textContent'), /not.*schedule DRAM/);
  await b.click('[data-node="sys.rni"]');
  assert.match(await b.evaluate('document.getElementById("detail-h").textContent'), /unverified/);
  assert.deepEqual(b.exceptions, []);
});

const EXPECT = {
  a: [['sys', []], ['noc', [['REQ', '0x08', '0x00', 1, 'ccw']]], ['hnf', []], ['noc', [['SNP', '0x00', '0x18', 3, 'cw']]], ['sys', []],
    ['noc', [['RSP', '0x18', '0x00', 3, 'ccw']]], ['noc', [['DAT', '0x18', '0x08', 2, 'ccw']]], ['noc', [['RSP', '0x08', '0x00', 1, 'ccw']]], ['hnf', []]],
  b: [['sys', []], ['noc', [['REQ', '0x08', '0x00', 1, 'ccw']]], ['hnf', []], ['noc', [['DAT', '0x00', '0x08', 1, 'cw']]], ['noc', [['RSP', '0x08', '0x00', 1, 'ccw']]], ['hnf', []]],
  c: [['sys', []], ['noc', [['REQ', '0x08', '0x10', 1, 'cw']]], ['hnf', []], ['noc', [['ERQ', '0x30', '0x38', 1, 'cw']]], ['mem', []], ['mem', []],
    ['noc', [['DAT', '0x38', '0x08', 5, 'cw']]], ['noc', [['RSP', '0x08', '0x30', 5, 'cw']]], ['hnf', []]],
};
const OPS = {
  a: ['AcquireBlock NtoB', 'ReadNotSharedDirty', 'SnpNotSharedDirtyFwd', 'CompData (SC, DCT)', 'SnpRespFwded SC · Fwd SC', 'CompAck'],
  b: ['ReadNotSharedDirty', 'CompData UC', 'CompData (UC)', 'CompAck'],
  c: ['ReadNotSharedDirty', 'ReadNoSnp', 'ReadNoSnp (ReturnNID = 0x08)', 'AR', 'R ×2', 'CompData (UC, DMT)', 'CompAck'],
};
for (const scn of ['a', 'b', 'c']) test(`transaction (${scn}) steps show verified routes and message types`, {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.click(`[data-scn="${scn}"]`);
  let s = await state(b);
  assert.equal(s.scn, scn); assert.equal(s.step, 0); assert.equal(s.steps, EXPECT[scn].length);
  assert.equal(await b.evaluate(`document.querySelector('[data-scn="${scn}"]').getAttribute('aria-pressed')`), 'true');
  assert.equal(await b.evaluate('document.getElementById("tx-prev").disabled'), true);
  assert.equal(await b.evaluate('document.querySelectorAll("#tx-steps li").length'), EXPECT[scn].length);
  const ops = new Set();
  for (let i = 0; i < EXPECT[scn].length; i++) {
    if (i) await b.click('#tx-next');
    s = await state(b);
    const [level, ring] = EXPECT[scn][i];
    assert.equal(s.step, i);
    assert.equal(s.level, level, `step ${i + 1} switches to ${level}`);
    assert.equal(s.stepLevel, level);
    assert.equal(s.drawnMsgs, s.msgs.length, `step ${i + 1}: one drawn path per message`);
    assert.ok(s.msgs.length >= 1);
    for (const m of s.msgs) ops.add(m.op);
    const got = s.msgs.filter(m => m.from).map(m => [m.ch, m.from, m.to, m.hops, m.dir]);
    assert.deepEqual(got, ring, `step ${i + 1} ring messages`);
    // Every block on each message path is lit, and the step list marks the current step.
    for (const m of s.msgs) {
      const nodes = m.from ? m.path.map(k => 'noc.' + ['hf0p0', 'cc0', 'hf1p0', 'cc1', 'ri', 'hi', 'hf1p1', 'sn', 'hf0p1', 'mn', 'pip'][k]) : m.path;
      for (const n of nodes) assert.ok(s.act.includes(n), `step ${i + 1}: ${n} highlighted`);
    }
    // Message labels are readable and do not cover each other or the block labels.
    const r = await b.evaluate(PROBE);
    assert.ok(r.minPx >= 11.5, `step ${i + 1}: smallest text ${r.minPx.toFixed(2)} px`);
    const ml = await b.evaluate(`(() => { const ls = [...document.querySelectorAll('#g-msgs .ml')].map(t => t.getBoundingClientRect()), ts = [...document.querySelectorAll('#g-nodes text, #g-edges text, #g-frames text')].map(t => t.getBoundingClientRect());
      const hit = (a, c) => a.left < c.right - 1 && c.left < a.right - 1 && a.top < c.bottom - 1 && c.top < a.bottom - 1;
      return ls.flatMap((a, i) => [...ls.slice(i + 1), ...ts].filter(c => hit(a, c)).map(() => i)); })()`);
    assert.deepEqual(ml, [], `step ${i + 1}: message labels clear of other labels`);
    assert.equal(await b.evaluate('document.querySelector("#tx-steps [aria-current=step]").dataset.step'), String(i));
    assert.match(await b.evaluate('document.getElementById("tx-count").textContent'), new RegExp(`Step ${i + 1} of ${EXPECT[scn].length}`));
    assert.ok((await b.evaluate('document.querySelectorAll("#tx-text .cites li").length')) >= 1, `step ${i + 1} cites its source`);
  }
  for (const op of OPS[scn]) assert.ok(ops.has(op), `message ${op} appears`);
  assert.equal(await b.evaluate('document.getElementById("tx-next").disabled'), true, 'Next disabled on the last step');
  // Previous goes back and redraws; a step can be picked from the list with the keyboard.
  await b.click('#tx-prev');
  assert.equal((await state(b)).step, EXPECT[scn].length - 2);
  await b.evaluate('document.querySelector("#tx-steps [data-step=\'1\']").focus()');
  await press(b, 'Enter');
  s = await state(b);
  assert.equal(s.step, 1); assert.equal(s.level, 'noc');
  // Switching level by hand keeps the transaction position and redraws the step on its own level.
  await b.click('#tab-mem');
  s = await state(b);
  assert.equal(s.step, 1); assert.equal(s.drawnMsgs, 0, 'no overlay on a level the step does not use');
  await b.click('#tab-noc');
  assert.equal((await state(b)).drawnMsgs, 1);
  // Each drawn flit starts at its sender and ends at its receiver.
  const ends = await b.evaluate(`(() => { const p = document.querySelector('#g-msgs path.m'), L = p.getTotalLength(), a = p.getPointAtLength(0), z = p.getPointAtLength(L);
    const c = id => { const r = document.querySelector('[data-node="' + id + '"] .box').getBBox(); return [r.x + r.width / 2, r.y + r.height / 2]; };
    const m = XZ.state().msgs[0], ids = ['hf0p0', 'cc0', 'hf1p0', 'cc1', 'ri', 'hi', 'hf1p1', 'sn', 'hf0p1', 'mn', 'pip'].map(x => 'noc.' + x);
    const d = (p, q) => Math.hypot(p.x - q[0], p.y - q[1]); const from = c(ids[m.path[0]]), to = c(ids[m.path.at(-1)]);
    return d(a, from) < d(a, to) && d(z, to) < d(z, from); })()`);
  assert.equal(ends, true, 'flit drawn from sender to receiver');
  // Exit clears overlays.
  await b.click('#tx-exit');
  s = await state(b);
  assert.equal(s.scn, null); assert.equal(s.drawnMsgs, 0);
  assert.equal(await b.evaluate('document.getElementById("svg").classList.contains("in-step")'), false);
  assert.deepEqual(b.exceptions, []);
});

test('play advances steps automatically and stops at the end', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.click('[data-scn="b"]');
  await b.evaluate('(() => { const r = setInterval; window.setInterval = (f) => r(f, 60); })()');
  await b.click('#tx-play');
  assert.equal(await b.evaluate('document.getElementById("tx-play").getAttribute("aria-pressed")'), 'true');
  await b.wait('XZ.state().step === XZ.state().steps - 1 && document.getElementById("tx-play").getAttribute("aria-pressed") === "false"');
  assert.equal((await state(b)).level, 'hnf');
  assert.deepEqual(b.exceptions, []);
});

test('reduced motion: flits are drawn without moving dots or line animation', {timeout: 30000}, async t => {
  const b = await open(1366, 800, [{name: 'prefers-reduced-motion', value: 'reduce'}]); t.after(() => b.close());
  await b.click('[data-scn="c"]');
  await b.click('#tx-next');
  assert.equal((await state(b)).drawnMsgs, 1);
  assert.equal(await b.evaluate('document.querySelectorAll("#g-msgs animateMotion").length'), 0);
  assert.equal(await b.evaluate('getComputedStyle(document.querySelector("#g-msgs path.m")).animationName'), 'none');
  assert.deepEqual(b.exceptions, []);
});

test('XS core level: frontend, CtrlBlock, three regions and MemBlock match the core parameters in Scala', {timeout: 30000}, async t => {
  // Independent reading of the source: issue-queue counts per scheduler and the sizes the page draws.
  const src = await readFile(new URL('src/main/scala/xiangshan/Parameters.scala', XS), 'utf8');
  const fe = f => readFile(new URL(`src/main/scala/xiangshan/frontend/${f}`, XS), 'utf8');
  const block = name => src.slice(src.indexOf(`val ${name} = {`), src.indexOf('numPregs', src.indexOf(`val ${name} = {`)));
  const iqs = name => block(name).match(/IssueBlockParams\(/g).length;
  const num = (text, re) => +text.match(re)[1];
  const want = {
    int: iqs('intSchdParams'), fp: iqs('fpSchdParams'), vec: iqs('vecSchdParams'),
    rob: num(src, /RobSize: Int = (\d+)/), decode: num(src, /DecodeWidth: Int = (\d+)/),
    intPreg: num(src, /intPreg: PregParams = IntPregParams\(\s*numEntries = (\d+)/), fpPreg: num(src, /fpPreg: PregParams = FpPregParams\(\s*numEntries = (\d+)/),
    vfPreg: num(src, /vfPreg: VfPregParams = VfPregParams\(\s*numEntries = (\d+)/), ldu: num(src, /LoadPipelineWidth: Int = (\d+)/),
    lq: num(src, /VirtualLoadQueueSize: Int = (\d+)/), sq: num(src, /StoreQueuePhysicalSize: Int = (\d+)/), sbuf: num(src, /StoreBufferSize: Int = (\d+)/),
    ftq: num(await fe('ftq/FtqParameters.scala'), /FtqSize:\s+Int = (\d+)/), ibuf: num(await fe('ibuffer/Parameters.scala'), /Size:\s+Int = (\d+)/),
  };
  assert.deepEqual([want.int, want.fp, want.vec], [13, 4, 6], 'issue queues per scheduler in Scala');
  assert.equal(block('intSchdParams').match(/ExeUnitParams\(\s*"ALU\d"/g).length, 6);
  assert.equal(block('intSchdParams').match(/"BJU\d"/g).length, 3);
  const b = await open(); t.after(() => b.close());
  await b.click('#tab-core');
  assert.equal((await state(b)).level, 'core');
  const text = await b.evaluate('document.getElementById("g-nodes").textContent');
  for (const s of [`Int issue queues ×${want.int}`, `FP issue queues ×${want.fp}`, `Vector issue queues ×${want.vec}`, `ROB${want.rob} · commit`, `${want.decode}-wide`,
    `Int regfile · ${want.intPreg}`, `FP regfile · ${want.fpPreg}`, `Vector regfile · ${want.vfPreg}`, `LDU ×${want.ldu}`, `LQ ${want.lq} · SQ ${want.sq}`,
    `${want.sbuf} lines`, `FTQ${want.ftq} entries`, `IBuffer${want.ibuf} entries`, 'ALU ×6 · BJU ×3'])
    assert.ok(text.includes(s), `core level shows "${s}"`);
  // Everything that leaves the core toward the L2 is drawn: ICache, DCache (two TL-C ports), page walker and uncached path.
  const toL2 = await b.evaluate(`[...document.querySelectorAll('#g-edges [data-b="core.l2"]')].map(g => g.dataset.a).sort()`);
  assert.deepEqual(toL2, ['core.dcache', 'core.icache', 'core.l2tlb', 'core.uncache']);
  // The frontend-to-backend flow is connected in order.
  const e = await b.evaluate(`[...document.querySelectorAll('#g-edges [data-edge]')].map(g => g.dataset.a + '>' + g.dataset.b)`);
  for (const pair of ['core.bpu>core.ftq', 'core.ftq>core.icache', 'core.icache>core.ifu', 'core.ifu>core.ibuf', 'core.ibuf>core.decode', 'core.decode>core.rename', 'core.rename>core.dispatch', 'core.dispatch>core.intiq', 'core.intrf>core.lsu', 'core.lsu>core.dcache'])
    assert.ok(e.includes(pair), pair);
  await b.click('[data-node="core.intiq"]');
  assert.match(await b.evaluate('document.getElementById("detail").textContent'), /ALU0\/BJU0.*18 entries.*STD0 and STD1 \(16\)/);
  // The system level points at this level.
  await b.click('#tab-sys'); await b.click('[data-node="sys.core0"]');
  assert.match(await b.evaluate('document.getElementById("detail").textContent'), /Level 2 shows the core's internals/);
  assert.deepEqual(b.exceptions, []);
});
