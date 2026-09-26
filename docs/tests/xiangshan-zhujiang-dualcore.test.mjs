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
const KEYS = {Enter: ['Enter', 13], ArrowRight: ['ArrowRight', 39], ArrowLeft: ['ArrowLeft', 37], ArrowDown: ['ArrowDown', 40], Escape: ['Escape', 27], Tab: ['Tab', 9], Home: ['Home', 36], End: ['End', 35]};
const press = (b, k) => key(b, k, KEYS[k][0], KEYS[k][1]);
const focused = b => b.evaluate('document.activeElement?.dataset?.node ?? document.activeElement?.id ?? null');
// Level changes zoom; geometry and overlays are checked once the camera lands.
const settle = b => b.wait('!XZ.state().flying');
const choose = async (b, scn) => { await b.click('#tx-pick'); await b.click(`#tx-menu [data-scn="${scn}"]`); await settle(b); };

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
  for (const z of s.querySelectorAll('#live .zoom')) for (const t of texts) if (hit(rect(z), rect(t))) overlaps.push('magnifier | ' + t.textContent);
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
  // The transaction player sits above the diagram, and its controls stay inside it with no overflow.
  await b.evaluate('XZ.startScenario("a")');
  const pl = await b.evaluate(`(() => { const p = document.getElementById('player'), r = p.getBoundingClientRect(), s = document.getElementById('svg').getBoundingClientRect();
    const out = [...p.querySelectorAll('button, #tx-track')].filter(e => { const q = e.getBoundingClientRect(); return q.left < r.left || q.right > r.right || q.top < r.top || q.bottom > r.bottom; }).map(e => e.id);
    return {above: r.bottom <= s.top, out, over: p.scrollWidth > p.clientWidth, track: document.getElementById('tx-track').getBoundingClientRect().width, row: (() => { const t = document.querySelector('.pl-transport').getBoundingClientRect(), m = document.querySelector('.pl-time').getBoundingClientRect(), c = (t.top + t.bottom) / 2; return c > m.top && c < m.bottom; })()}; })()`);
  assert.ok(pl.above, 'player above the diagram');
  assert.deepEqual(pl.out, [], 'player controls inside the toolbar');
  assert.equal(pl.over, false, 'player does not overflow');
  assert.ok(pl.track >= 120, `timeline is ${pl.track} px wide`);
  assert.ok(pl.row, 'transport buttons and timeline share one row');
  await b.evaluate('XZ.exitScenario()');
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
  for (const name of ['Choose a transaction', 'Previous step', 'Next step', 'Play', 'Exit transaction', 'Toggle colour theme'])
    assert.ok(tree.some(n => n.role?.value === 'button' && n.name?.value === name), `button "${name}"`);
  assert.ok(tree.some(n => n.role?.value === 'region' && n.name?.value === 'Guided transactions'), 'player is a named region');
  const slider = tree.find(n => n.role?.value === 'slider' && n.name?.value === 'Transaction step');
  assert.ok(slider, 'timeline is a named slider');
  // The transaction menu opens from the keyboard, lists every transaction as a radio item and returns focus on Escape.
  await b.evaluate('document.getElementById("tx-pick").focus()');
  await press(b, 'ArrowDown');
  assert.equal(await b.evaluate('document.getElementById("tx-pick").getAttribute("aria-expanded")'), 'true');
  tree = await ax();
  assert.deepEqual(tree.filter(n => n.role?.value === 'menuitemradio').map(n => n.name.value), ['(a) L1 miss that hits in the other core', '(b) LLC hit', '(c) LLC miss to memory']);
  assert.equal(await b.evaluate('document.activeElement.dataset.scn'), 'a');
  await press(b, 'Escape');
  assert.equal(await b.evaluate('document.getElementById("tx-menu").hidden'), true);
  assert.equal(await focused(b), 'tx-pick');
  await press(b, 'Enter');
  assert.equal(await b.evaluate('document.activeElement.dataset.scn'), 'a', 'Enter opens the menu on the current item');
  await press(b, 'ArrowDown');
  await press(b, 'Enter');
  let s = await state(b);
  assert.equal(s.scn, 'b'); assert.equal(s.step, 0);
  assert.equal(await focused(b), 'tx-pick', 'focus returns to the menu button');
  tree = await ax();
  const sl = tree.find(n => n.role?.value === 'slider');
  assert.equal(sl.value?.value, 1); assert.equal(prop(sl, 'valuemax'), 6);
  // Chrome's CDP tree leaves valuetext empty, so the attribute is checked in the DOM.
  assert.equal(await b.evaluate('document.getElementById("tx-track").getAttribute("aria-valuetext")'), 'Step 1 of 6: Hart 0 misses in L1D and L2');
  await b.evaluate('XZ.exitScenario()');
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
  await settle(b);
  await b.click('[data-node="mem.axib"]');
  assert.match(await b.evaluate('document.getElementById("detail").textContent'), /TgtID.*ReturnNID/);
  await b.click('#tab-sys'); await settle(b);
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
  await choose(b, scn);
  let s = await state(b);
  assert.equal(s.scn, scn); assert.equal(s.step, 0); assert.equal(s.steps, EXPECT[scn].length);
  assert.equal(await b.evaluate(`document.querySelector('#tx-menu [data-scn="${scn}"]').getAttribute('aria-checked')`), 'true');
  assert.equal(await b.evaluate('document.getElementById("tx-menu").hidden'), true, 'choosing closes the menu');
  assert.equal(await b.evaluate('document.getElementById("tx-pick-lbl").textContent'), await b.evaluate(`XZ.SCEN.${scn}.name`));
  assert.equal(await b.evaluate('document.getElementById("tx-prev").disabled'), true);
  assert.equal(await b.evaluate('document.querySelectorAll("#tx-track .seg").length'), EXPECT[scn].length, 'one timeline segment per step');
  const ops = new Set();
  for (let i = 0; i < EXPECT[scn].length; i++) {
    const [level, ring] = EXPECT[scn][i];
    if (i) {
      await b.click('#tx-next');
      s = await state(b);
      // A step on another level zooms there first and draws its flits once the camera lands.
      if (level !== EXPECT[scn][i - 1][0]) {
        assert.equal(s.flying, true, `step ${i + 1} zooms to ${level}`); assert.equal(s.drawnMsgs, 0);
        // Zooming in, the block entered glows and its blocks are not dimmed like the rest of the step.
        const outward = {noc: ['sys'], hnf: ['noc', 'sys'], mem: ['sys']}[EXPECT[scn][i - 1][0]]?.includes(level);
        const lit = await b.evaluate(`(() => { const l = [...document.querySelectorAll('#scene .ghost .node.lit')], o = [...document.querySelectorAll('#scene .ghost .node:not(.lit):not(.act)')];
          return {n: l.length, bright: l.every(g => getComputedStyle(g).opacity === '1'), dim: o.every(g => getComputedStyle(g).opacity === '0.5'), glow: document.querySelectorAll('#scene .aura.to').length}; })()`);
        if (outward) assert.deepEqual([lit.n, lit.glow], [0, 0], `step ${i + 1}: nothing glows when zooming out`);
        else assert.ok(lit.n > 0 && lit.bright && lit.dim && lit.glow > 0, `step ${i + 1}: the entered block glows and stands out ${JSON.stringify(lit)}`);
      }
      await settle(b);
    }
    s = await state(b);
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
    assert.equal(await b.evaluate('document.getElementById("tx-track").getAttribute("aria-valuenow")'), String(i + 1));
    assert.deepEqual(await b.evaluate('[...document.querySelectorAll("#tx-track .seg")].map(g => g.classList.contains("done"))'), EXPECT[scn].map((_, j) => j <= i), 'timeline filled up to the current step');
    assert.equal(await b.evaluate('document.querySelector("#tx-track .seg.head").dataset.step'), String(i), 'playhead on the current step');
    assert.match(await b.evaluate('document.getElementById("tx-count").textContent'), new RegExp(`Step ${i + 1} of ${EXPECT[scn].length}`));
    assert.ok((await b.evaluate('document.querySelectorAll("#tx-text .cites li").length')) >= 1, `step ${i + 1} cites its source`);
  }
  for (const op of OPS[scn]) assert.ok(ops.has(op), `message ${op} appears`);
  assert.equal(await b.evaluate('document.getElementById("tx-next").disabled'), true, 'Next disabled on the last step');
  // Previous goes back and redraws; clicking a timeline segment jumps to it; the timeline steps by keyboard.
  await b.click('#tx-prev'); await settle(b);
  assert.equal((await state(b)).step, EXPECT[scn].length - 2);
  await b.click('#tx-track .seg[data-step="2"]'); await settle(b);
  assert.equal((await state(b)).step, 2);
  const tip = await b.evaluate(`(() => { const t = document.getElementById('tx-tip'), r = t.getBoundingClientRect(), p = document.getElementById('player').getBoundingClientRect(); return {hidden: t.hidden, text: t.textContent, inside: r.left >= p.left && r.right <= p.right}; })()`);
  assert.equal(tip.hidden, false);
  assert.equal(tip.text, await b.evaluate(`'Step 3 · ' + document.querySelector('[data-level="' + XZ.SCEN.${scn}.steps[2].level + '"]').textContent.slice(4) + XZ.SCEN.${scn}.steps[2].title`), 'tooltip names the step under the pointer');
  assert.ok(tip.inside, 'tooltip stays inside the player');
  assert.equal(await focused(b), 'tx-track');
  await press(b, 'End');
  assert.equal((await state(b)).step, EXPECT[scn].length - 1);
  await press(b, 'Home');
  assert.equal((await state(b)).step, 0);
  await press(b, 'ArrowRight'); await settle(b);
  s = await state(b);
  assert.equal(s.step, 1); assert.equal(s.level, 'noc');
  // Switching level by hand keeps the transaction position and redraws the step on its own level.
  await b.click('#tab-mem'); await settle(b);
  s = await state(b);
  assert.equal(s.step, 1); assert.equal(s.drawnMsgs, 0, 'no overlay on a level the step does not use');
  await b.click('#tab-noc'); await settle(b);
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
  assert.equal(await b.evaluate('document.getElementById("tx-narr").hidden'), true, 'narration hidden without a transaction');
  assert.equal(await b.evaluate('document.getElementById("tx-exit").disabled'), true);
  assert.equal(await b.evaluate('document.getElementById("svg").classList.contains("in-step")'), false);
  assert.deepEqual(b.exceptions, []);
});

test('play advances steps automatically and stops at the end', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await choose(b, 'b');
  await b.evaluate('(() => { const r = setTimeout; window.setTimeout = (f, ms) => r(f, Math.min(ms, 60)); })()');
  await b.click('#tx-play');
  const label = 'document.getElementById("tx-play").getAttribute("aria-label")';
  assert.equal(await b.evaluate(label), 'Pause');
  assert.equal(await b.evaluate('document.querySelector("#tx-track .seg.filling")?.dataset.step'), '1', 'the next segment fills while playing');
  await b.wait(`XZ.state().step === XZ.state().steps - 1 && ${label} === "Play"`);
  assert.equal((await state(b)).level, 'hnf');
  assert.equal(await b.evaluate('document.querySelectorAll("#tx-track .seg.filling").length'), 0);
  // Play at the end replays from the start; Pause stops where it is.
  await b.evaluate('window.setTimeout = () => 0');
  await b.click('#tx-play');
  let s = await state(b);
  assert.equal(s.step, 0); assert.equal(s.playing, true);
  await b.click('#tx-play');
  s = await state(b);
  assert.equal(s.step, 0); assert.equal(s.playing, false); assert.equal(await b.evaluate(label), 'Play');
  assert.deepEqual(b.exceptions, []);
});

test('reduced motion: flits are drawn without moving dots or line animation', {timeout: 30000}, async t => {
  const b = await open(1366, 800, [{name: 'prefers-reduced-motion', value: 'reduce'}]); t.after(() => b.close());
  await choose(b, 'c');
  await b.click('#tx-next');
  assert.equal((await state(b)).drawnMsgs, 1);
  assert.equal(await b.evaluate('document.querySelectorAll("#g-msgs animateMotion").length'), 0);
  assert.equal(await b.evaluate('getComputedStyle(document.querySelector("#g-msgs path.m")).animationName'), 'none');
  assert.deepEqual(b.exceptions, []);
});

// Records every animation frame of the current zoom: camera scale, and each drawn level with its opacity.
const SAMPLE = `new Promise(done => { const out = [], sc = document.getElementById('scene');
  const f = () => { const m = sc.transform.baseVal.consolidate()?.matrix;
    const auras = [...sc.querySelectorAll('.aura')].map(a => ({kind: a.classList.contains('to') ? 'to' : 'from', key: a.dataset.aura, in: a.closest('#scene > g').querySelector('[data-node]').dataset.node.split('.')[0],
      label: a.querySelector('text')?.textContent ?? null, box: (r => [r.x, r.y, r.width, r.height].map(Math.round))(a.querySelector('.edge').getBBox())}));
    out.push({ms: performance.now(), auras, k: m ? m.a : 1, vb: document.getElementById('svg').getAttribute('viewBox'), layers: [...sc.children].map(g => ({live: g.id === 'live', hidden: g.getAttribute('aria-hidden'),
      level: g.querySelector('[data-node]')?.dataset.node.split('.')[0], op: g.style.opacity === '' ? 1 : +g.style.opacity, focusable: g.querySelectorAll('[tabindex]').length}))});
    if (XZ.state().flying) requestAnimationFrame(f); else done(out); };
  f(); })`;
const rising = (xs, tol = 1e-6) => xs.every((x, i) => !i || x >= xs[i - 1] - tol);
test('changing level zooms smoothly through the levels between and lands on the plain level', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  // System → ring: one zoom into the ZhuJiang frame while the system fades and the ring appears.
  await b.click('#tab-noc');
  assert.equal((await state(b)).flying, true, 'the tab starts a zoom');
  let fr = await b.evaluate(SAMPLE);
  const fly = fr.slice(0, -1), end = fr.at(-1), rect = await b.evaluate('XZ.placeOf("noc")');
  assert.ok(fly.length >= 8, `${fly.length} frames`);
  // First the camera holds while the ZhuJiang frame it will enter glows, labelled with the level.
  assert.deepEqual(fly[0].auras, [{kind: 'to', key: 'noc', in: 'sys', label: 'NoC ring', box: [13, 293, 974, 220]}], 'the block to be entered is outlined first');
  const hold = fly.filter(f => f.ms - fly[0].ms < 550);
  assert.ok(hold.length >= 5 && hold.every(f => f.k === 1), 'the camera waits while the block lights up');
  assert.ok(fly[0].k < 1.15 && fly.at(-1).k > 2.6, `scale ${fly[0].k} → ${fly.at(-1).k}`);
  assert.ok(rising(fly.map(f => f.k)), 'zoom only moves inward');
  assert.ok(fly.every((f, i) => !i || f.k / fly[i - 1].k < 1.25), 'no jumps between frames');
  assert.deepEqual(fly[0].layers.map(l => [l.level, l.live]), [['sys', false], ['noc', true]], 'the ring is drawn over the system it details');
  const op = lv => fly.map(f => f.layers.find(l => l.level === lv).op);
  assert.ok(op('noc')[0] < .05 && op('noc').at(-1) > .95 && rising(op('noc')) && op('sys').every(o => o === 1), 'the ring fades in on its card over the system');
  assert.equal(await b.evaluate('document.querySelectorAll("#scene .card").length'), 0, 'the card goes with the zoom');
  assert.ok(fly.every(f => f.layers.every(l => l.live || (l.hidden === 'true' && l.focusable === 0))), 'levels passed through are hidden from assistive technology and focus');
  assert.deepEqual(end.layers.map(l => l.live), [true], 'only the ring remains after landing');
  assert.equal(end.k, 1); assert.equal(end.vb, '0 0 1000 660');
  assert.equal(rect.s.toFixed(3), (206 / 660).toFixed(3), 'the ring fills the ZhuJiang frame height');
  // Ring → memory path: the camera pulls back to the system between them and zooms in again.
  await b.click('#tab-mem');
  fr = await b.evaluate(SAMPLE);
  const k = fr.slice(0, -1).map(f => f.k);
  assert.ok(Math.min(...k) < 0.7 * Math.min(k[0], k.at(-1)), `pulls back between siblings: ${Math.min(...k).toFixed(2)}`);
  assert.deepEqual(fr[0].layers.map(l => l.level).sort(), ['mem', 'noc', 'sys']);
  assert.deepEqual(fr[0].auras.map(a => [a.kind, a.key, a.in, a.label]).sort(), [['from', 'noc', 'sys', null], ['to', 'mem', 'sys', 'Memory path']], 'the block left is outlined, the block entered glows');
  assert.ok(fr[1].k < fr[0].k, 'between siblings the camera pulls back at once');
  const ring = fr.slice(0, -1).map(f => f.layers.find(l => l.level === 'noc').op), mem = fr.slice(0, -1).map(f => f.layers.find(l => l.level === 'mem').op);
  assert.ok(ring[0] > .95 && mem.at(-1) > .95 && ring.some((o, i) => o < .05 && mem[i] < .05), 'the ring card fades out before the memory card fades in, over the system');
  assert.equal(fr.at(-1).vb, '0 0 1000 650');
  // Memory path → HNF bank 0 passes through the system and the ring; the bank grows out of its ring box.
  await b.click('#tab-hnf');
  fr = await b.evaluate(SAMPLE);
  assert.deepEqual(fr[0].layers.map(l => l.level), ['sys', 'mem', 'noc', 'hnf'], 'levels drawn parent below child');
  assert.ok(fr.at(-2).k > 15, `deep zoom into the bank: ${fr.at(-2).k.toFixed(1)}`);
  // Bank switch pulls back to the ring and zooms into the other bank.
  await b.click('[data-bank="1"]');
  assert.equal((await state(b)).bank, 1);
  fr = await b.evaluate(SAMPLE);
  assert.deepEqual(fr[0].layers.map(l => l.level), ['noc', 'hnf', 'hnf']);
  assert.deepEqual(fr[0].auras.map(a => [a.kind, a.key, a.label]).sort(), [['from', 'hnf0', null], ['to', 'hnf1', 'HNF bank 1']]);
  assert.ok(fr.some(f => f.layers[1].op < .05 && f.layers[2].op < .05), 'the ring is shown between the banks');
  // Zooming out leaves a fading outline of the block we came from on the landed level.
  await b.click('#tab-noc'); await settle(b);
  assert.deepEqual(await b.evaluate('[...document.querySelectorAll("#live > .aura.out")].map(a => a.dataset.aura)'), ['hnf1']);
  await b.wait('!document.querySelector("#live > .aura")');
  await b.click('#tab-hnf'); await settle(b);
  // After landing, blocks are clickable at their drawn place.
  await b.click('[data-node="hnf.xbar"]');
  assert.equal((await state(b)).sel, 'hnf.xbar');
  assert.deepEqual(b.exceptions, []);
});

test('double-clicking an outlined block zooms into its level exactly as its tab does', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  // Clicking a tab scrolls it into view, so each point is taken with the diagram scrolled back.
  const at = (x, y) => b.evaluate(`(() => { document.getElementById('svg').scrollIntoView({block: 'center', behavior: 'instant'}); const p = new DOMPoint(${x}, ${y}).matrixTransform(document.getElementById('g-frames').getScreenCTM()); return {x: p.x, y: p.y}; })()`);
  const centre = sel => b.evaluate(`(() => { document.getElementById('svg').scrollIntoView({block: 'center', behavior: 'instant'}); const r = document.querySelector(${JSON.stringify(sel)}).getBoundingClientRect(); return {x: r.x + r.width / 2, y: r.y + r.height / 2}; })()`);
  const mouse = (type, p, clickCount = 1) => b.send('Input.dispatchMouseEvent', {type, button: type === 'mouseMoved' ? 'none' : 'left', clickCount, ...p});
  const dbl = async p => { for (const n of [1, 2]) { await mouse('mousePressed', p, n); await mouse('mouseReleased', p, n); } };
  const zoomBy = async act => { await act(); const s = await state(b); await settle(b); return s.zoom; };
  const hover = async p => { await mouse('mouseMoved', p); return b.evaluate('[...document.querySelectorAll("#live .nav.hot")].map(g => g.dataset.nav)'); };
  const navs = () => b.evaluate(`[...document.querySelectorAll('#live .nav')].map(g => [g.dataset.nav, g.querySelectorAll('rect').length, g.querySelector('.zoom').getAttribute('aria-label')])`);
  // The system outlines tile 0 (the core), the ZhuJiang frame (the ring) and the three blocks of the memory path.
  assert.deepEqual(await navs(), [['core', 1, 'Zoom into XS core'], ['noc', 1, 'Zoom into NoC ring'], ['mem', 3, 'Zoom into Memory path']]);
  const colours = await b.evaluate(`(() => { const c = s => getComputedStyle(document.querySelector(s)).stroke, p = document.body.appendChild(document.createElement('i'));
    p.style.color = 'var(--nav)'; const nav = getComputedStyle(p).color; p.style.color = 'var(--accent)'; const accent = getComputedStyle(p).color; p.remove();
    return {nav, accent, outline: c('#live .nav rect'), frame: c('#live .frame rect'), box: c('#live .box')}; })()`);
  assert.equal(colours.outline, colours.nav, 'outlines use the navigation colour');
  assert.equal(new Set([colours.nav, colours.accent, colours.frame, colours.box]).size, 4, 'the navigation colour differs from frames, blocks and the accent');
  const empty = await at(970, 495);   // inside the ZhuJiang frame, beside the MN
  assert.equal(await b.evaluate(`document.elementFromPoint(${empty.x}, ${empty.y}).getAttribute('class')`), 'nav-area');
  // Hovering shows which level a double-click enters: the SN belongs to the memory path although it sits on the ring.
  assert.deepEqual(await hover(await centre('[data-node="sys.l1d0"]')), ['core']);
  assert.deepEqual(await hover(await centre('[data-node="sys.sn"]')), ['mem']);
  assert.deepEqual(await hover(empty), ['noc']);
  assert.deepEqual(await hover(await centre('[data-node="sys.l1d1"]')), [], 'tile 1 leads nowhere');
  // Double-clicks make the same flight as the tabs.
  const viaTab = {};
  for (const lv of ['core', 'noc', 'mem']) { viaTab[lv] = await zoomBy(() => b.click(`#tab-${lv}`)); await b.evaluate('XZ.setLevel("sys")'); }
  for (const [lv, p] of [['core', () => centre('[data-node="sys.l1d0"]')], ['mem', () => centre('[data-node="sys.socxbar"]')], ['noc', () => at(970, 495)]]) {
    assert.deepEqual(await zoomBy(async () => dbl(await p())), viaTab[lv], `double-click into ${lv}`);
    const s = await state(b);
    assert.equal(s.level, lv); assert.equal(s.sel, null, 'the clicks of a double-click leave no selection');
    assert.deepEqual(await navs(), lv === 'noc' ? [['hnf0', 1, 'Zoom into HNF bank 0'], ['hnf1', 1, 'Zoom into HNF bank 1']] : [], `${lv}: outlines`);
    if (lv !== 'noc') await b.evaluate('XZ.setLevel("sys")');
  }
  // Ring → bank 1 selects the bank and flies as the HNF tab does with bank 1 chosen.
  await b.evaluate('XZ.setBank(1)');
  const bank1 = await zoomBy(() => b.click('#tab-hnf'));
  await b.evaluate('XZ.setLevel("noc"); XZ.setBank(0)');
  assert.deepEqual(await zoomBy(async () => dbl(await centre('[data-node="noc.bank1"]'))), bank1);
  assert.deepEqual(bank1, {from: 'noc', to: 'hnf1', ms: bank1.ms});
  assert.equal((await state(b)).bank, 1);
  // A block no level details does nothing.
  await b.evaluate('XZ.setLevel("sys")');
  await dbl(await centre('[data-node="sys.periph"]'));
  assert.equal((await state(b)).flying, false); assert.equal((await state(b)).level, 'sys');
  // The magnifier is a button: a click or Enter makes the same zoom, and Enter leaves focus on the level's tab.
  assert.deepEqual(await zoomBy(() => b.click('#live [data-nav="noc"] .zoom')), viaTab.noc);
  await b.evaluate('XZ.setLevel("sys"); document.querySelector(\'#live [data-nav="mem"] .zoom\').focus()');
  assert.deepEqual(await zoomBy(() => press(b, 'Enter')), viaTab.mem);
  assert.equal(await focused(b), 'tab-mem');
  assert.deepEqual(b.exceptions, []);
});

test('a transaction replays on one zooming view and waits for each zoom before the next step', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await choose(b, 'c');
  // Record the level and whether the camera is moving at every frame of a full replay, with short dwell.
  await b.evaluate('XZ.setDwell(250)');
  const log = b.evaluate(`new Promise(done => { const out = []; let idle = 0; const f = () => { const s = XZ.state();
    if (s.playing || out.length) out.push([s.step, s.level, s.flying, s.drawnMsgs]);
    idle = out.length && !s.playing && !s.flying ? idle + 1 : 0;
    if (idle < 5) requestAnimationFrame(f); else done(out); }; requestAnimationFrame(f); })`);
  await b.click('#tx-play');
  const out = await log;
  const steps = [...new Set(out.map(o => o[0]))];
  assert.deepEqual(steps, [...Array(9).keys()], 'every step is shown in order');
  for (const i of steps) {
    const at = out.filter(o => o[0] === i), still = at.filter(o => !o[2]);
    assert.ok(still.length >= 3, `step ${i + 1} is shown at rest before the next`);
    assert.ok(still.every(o => o[3] >= 1), `step ${i + 1} draws its flits once landed`);
    assert.ok(at.filter(o => o[2]).every(o => o[3] === 0), `step ${i + 1}: no flits while zooming`);
  }
  assert.ok(out.some(o => o[2]), 'steps on other levels zoom');
  assert.deepEqual(b.exceptions, []);
});

test('reduced motion: level changes are immediate', {timeout: 30000}, async t => {
  const b = await open(1366, 800, [{name: 'prefers-reduced-motion', value: 'reduce'}]); t.after(() => b.close());
  await b.click('#tab-noc');
  const s = await state(b);
  assert.equal(s.level, 'noc'); assert.equal(s.flying, false);
  assert.equal(await b.evaluate('document.querySelectorAll("#scene > g").length'), 1);
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
  await b.click('#tab-core'); await settle(b);
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
  await b.click('#tab-sys'); await settle(b); await b.click('[data-node="sys.core0"]');
  assert.match(await b.evaluate('document.getElementById("detail").textContent'), /Level 2 shows the core's internals/);
  assert.deepEqual(b.exceptions, []);
});
