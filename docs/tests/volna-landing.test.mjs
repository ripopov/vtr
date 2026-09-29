// node --test --test-concurrency=1 docs/tests/volna-landing.test.mjs   (after volna/volna/web/build.sh)
// Owns a headless browser and a loopback server over the repository; the
// assertions are the gate. The live-demo test needs the WASM bundle and fails
// when it is missing; the others hide it to check the page alone.
import assert from 'node:assert/strict';
import {access, readFile} from 'node:fs/promises';
import {join, normalize, sep} from 'node:path';
import {test} from 'node:test';
import {fileURLToPath} from 'node:url';
import {browserTest, until} from '../../volna/volna/tools/browser-test.mjs';

const root = fileURLToPath(new URL('../..', import.meta.url));
const PAGE = '/docs/volna-landing.html';
const DIST = '/volna/volna/web/dist/';
const TYPES = {'.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.wasm': 'application/wasm',
  '.json': 'application/json', '.svg': 'image/svg+xml', '.ttf': 'font/ttf', '.md': 'text/markdown', '.vtr': 'application/octet-stream'};
const bench = JSON.parse(await readFile(join(root, 'bench/results/latest/results.json'), 'utf8'));
const html = await readFile(join(root, 'docs/volna-landing.html'), 'utf8');
const settle = 'new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))';

/** Every repository file at its path; `withViewer: false` hides the WASM bundle. */
function repository({withViewer}) {
  return new Proxy({}, {get(_, path) {
    if (typeof path !== 'string') return undefined;
    if (path === '/') return {type: 'text/html', body: '<!doctype html><title>root</title>'};
    if (!withViewer && path.startsWith(DIST)) return undefined;
    const file = normalize(join(root, decodeURIComponent(path)));
    if (!file.startsWith(root) || file.endsWith(sep)) return undefined;
    return {type: TYPES[file.slice(file.lastIndexOf('.'))] ?? 'application/octet-stream', body: () => readFile(file)};
  }});
}

async function open({width = 1440, height = 900, withViewer = false} = {}) {
  const b = await browserTest(repository({withViewer}), {graphics: withViewer, ready: `location.protocol === 'http:'`});
  try {
    await b.send('Emulation.setDeviceMetricsOverride', {width, height, deviceScaleFactor: 1, mobile: false});
    const origin = await b.evaluate('location.origin');
    await b.send('Page.navigate', {url: origin + PAGE});
    await b.wait(`document.readyState === 'complete' && !!window.volnaDemo`);
    await b.evaluate('document.fonts.ready.then(() => true)');
    await b.evaluate(settle);
    return b;
  } catch (error) { await b.close(); throw error; }
}

test('the page opens with the slogan, then the live demo, then the pitch ideas and the feature list', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  const v = await b.evaluate(`(() => {
    const main = document.querySelector('main');
    const blocks = [...main.children].map(e => e.id || e.className);
    return {h1: document.querySelector('h1').textContent, firstHeading: main.querySelector('h1,h2').tagName,
      lede: document.querySelector('.lede').textContent, blocks, title: document.title,
      frame: document.getElementById('volna').title, h2: [...document.querySelectorAll('section.block h2')].map(e => e.textContent),
      features: [...document.querySelectorAll('.feature h3')].map(e => e.textContent),
      fonts: [...document.fonts].map(f => f.family.replace(/"/g, '') + ' ' + f.weight + ' ' + f.status).sort()};
  })()`);
  assert.equal(v.h1, 'Level up your traces.');
  assert.equal(v.firstHeading, 'H1', 'the slogan opens the page');
  assert.ok(v.lede.startsWith('See your hardware run at the level you designed it.'));
  assert.deepEqual(v.blocks, ['hero', 'demo', 'strip', 'level-up', 'down-up', 'one-file', 'five-viewers', 'features', 'closing']);
  assert.equal(v.frame, 'Volna viewer showing landing.vtr');
  assert.deepEqual(v.h2, ['Level up your traces', 'Optimized down. Lifted back up.', 'Two formats. One modern file.',
    'Five viewers. One modern framework.', 'One recording. Every view of it.']);
  assert.equal(v.features.length, 12);
  assert.deepEqual(v.fonts, ['Inter 400 loaded', 'Inter 600 loaded', 'JetBrains Mono 400 loaded']);
  assert.ok(!/(src|href)="https?:(?!\/\/github\.com\/ripopov\/vtr")/.test(html), 'no network resources besides the repository link');
  assert.deepEqual(b.exceptions, []);
});

test('each pitch slide reappears with its own headline, subtitle and ideas', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  const v = await b.evaluate(`(async () => {
    const pitch = new DOMParser().parseFromString(await (await fetch('/docs/vtr-pitch.html')).text(), 'text/html');
    const slide = id => { const s = pitch.getElementById(id); return {title: s.querySelector('h1').textContent, sub: s.querySelector('.sub').textContent,
      words: [...s.querySelectorAll('svg text')].map(e => e.textContent.trim()).join(' | ')}; };
    const section = id => { const s = document.getElementById(id); return {title: s.querySelector('h2').textContent, sub: s.querySelector('.sub').textContent}; };
    const texts = sel => [...document.querySelectorAll(sel)].map(e => e.textContent.trim());
    return {
      slides: ['level-up', 'down-up', 'one-file', 'everywhere'].map(slide),
      sections: ['level-up', 'down-up', 'one-file', 'five-viewers'].map(section),
      tags: [...pitch.querySelectorAll('#level-up .copy .tag')].map(e => e.textContent), ours: texts('#level-up .tag'),
      sentences: [...pitch.querySelectorAll('#level-up .copy p.s')].map(e => e.textContent), steps: texts('#level-up .step > p').slice(1),
      input: pitch.querySelector('#level-up .copy p:not(.s)')?.textContent ?? '', inputOurs: texts('#level-up .step > p')[0],
      levels: texts('#down-up .lvl b').concat(texts('#down-up .lvl span')),
      formats: texts('#one-file .fcard li').concat(texts('#one-file .sections span'), texts('#one-file .fcard .by')),
      viewers: texts('#five-viewers .viewer h3').concat(texts('#five-viewers .viewer .by'), texts('#five-viewers .viewer li'),
        texts('#five-viewers .becomes b'), texts('#five-viewers .target h3').map(t => t.replace(' this page', '')), texts('#five-viewers .target p')),
    };
  })()`);
  v.slides.forEach((s, i) => {
    assert.equal(v.sections[i].title, s.title);
    assert.equal(v.sections[i].sub, s.sub);
  });
  assert.deepEqual(v.ours, v.tags);
  assert.deepEqual(v.steps, v.sentences);
  // Slides break phrases over SVG lines; compare words, not line breaks or punctuation.
  const flat = text => ` ${text.replace(/[|,;]/g, ' ').replace(/\s+/g, ' ').trim()} `;
  const [, downUp, oneFile, everywhere] = v.slides;
  for (const word of v.levels) assert.ok(flat(downUp.words).includes(flat(word)), `slide 2 has ${word}`);
  for (const word of v.formats) assert.ok(flat(oneFile.words).includes(flat(word)), `slide 3 has ${word}`);
  for (const word of v.viewers) assert.ok(flat(everywhere.words).includes(flat(word)), `slide 4 has ${word}`);
  assert.equal(v.levels.length, 16);
  assert.equal(v.viewers.length, 5 * 5 + 6);
});

test('benchmark rows are recomputed from the results and drawn to scale', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  const c910 = bench.rtl.find(r => r.workload === 'c910_coremark'), tlm = bench.tx.find(r => r.workload === 'tlm_1m');
  const fstW = ['fstapi-lz4', 'fstapi-zlib', 'fstcpp-lz4'].map(k => c910.writers[k]), ftrW = ['ftr-lz4', 'ftr-raw'].map(k => tlm.writers[k]);
  const rd = c910.readers, min = a => Math.min(...a);
  const MiB = x => `${(x / 2 ** 20).toFixed(x >= 100 * 2 ** 20 ? 0 : 1)} MiB`, s = x => `${x.toFixed(x < 1 ? 2 : 1)} s`, ms = x => `${(x * 1000).toFixed(1)} ms`;
  const rows = [
    [min(fstW.map(w => w.bytes)), c910.writers['vtr-rust'].bytes, 'fst', 'smaller file', MiB],
    [min(fstW.map(w => w.wall_s)), c910.writers['vtr-rust'].wall_s, 'fst', 'faster write', s],
    [min([rd.vs_fstapi_zlib.load_1.fst_wellen, rd.fstapi_reader_zlib.load_1_s, rd.fstapi_reader_lz4.load_1_s]), rd.vs_fstapi_zlib.load_1.vtr, 'fst', 'faster read', ms],
    [min(ftrW.map(w => w.bytes)), tlm.writers['vtr-rust'].bytes, 'ftr', 'smaller file', MiB],
    [min(ftrW.map(w => w.wall_s)), tlm.writers['vtr-rust'].wall_s, 'ftr', 'faster write', s]];
  const bars = await b.evaluate(`[...document.querySelectorAll('#bench .bar')].map(bar => {
    const w = sel => bar.querySelector(sel).getBoundingClientRect();
    return {them: +bar.dataset.them, ours: +bar.dataset.ours, vs: bar.dataset.vs, ratio: bar.querySelector('.ratio b').textContent,
      says: bar.querySelector('.ratio span').textContent, labels: [...bar.querySelectorAll('.fill')].map(e => e.textContent),
      tw: w('.fill.them').width, ow: w('.fill.ours').width, tx: w('.fill.them').x, ox: w('.fill.ours').x, gx: w('.ghost').x, gw: w('.ghost').width};
  })`);
  assert.equal(bars.length, rows.length);
  rows.forEach(([them, ours, vs, says, unit], i) => {
    const bar = bars[i];
    assert.equal(bar.them, them, `row ${i}: competitor value`);
    assert.equal(bar.ours, ours, `row ${i}: VTR value`);
    assert.equal(bar.vs, vs);
    assert.equal(bar.ratio, `${(them / ours).toFixed(1)}×`);
    assert.equal(bar.says, says);
    assert.deepEqual(bar.labels, [`${vs.toUpperCase()} ${unit(them)}`, `VTR ${unit(ours)}`]);
    assert.equal(bar.tx, bar.ox, `row ${i}: one axis`);
    assert.ok(Math.abs(bar.ow / bar.tw - ours / them) < 0.002, `row ${i}: VTR drawn to scale`);
    assert.ok(Math.abs(bar.gx + bar.gw - (bar.tx + bar.tw)) < 1 && Math.abs(bar.gx - (bar.ox + bar.ow)) < 1, `row ${i}: the saving fills the gap`);
  });
  const signals = await b.evaluate(`document.querySelector('.bench-note').textContent`);
  assert.ok(signals.includes(`${c910.info.signals.toLocaleString('en-US')} signals`));
});

test('every width fits without sideways scrolling; links and resources resolve; tips switch', {timeout: 90000}, async t => {
  for (const [width, height] of [[390, 844], [768, 1024], [1024, 768], [1440, 900], [1920, 1080]]) {
    const b = await open({width, height});
    try {
      const v = await b.evaluate(`(() => {
        const stage = document.getElementById('stage').getBoundingClientRect(), frame = document.getElementById('volna').getBoundingClientRect();
        return {scroll: document.documentElement.scrollWidth, inner: innerWidth, stage: [stage.width, stage.height], frame: [frame.width, frame.height],
          wide: [...document.querySelectorAll('main *')].filter(e => { const r = e.getBoundingClientRect(); return r.width && r.right > innerWidth + 0.5 && !e.closest('#stage'); }).map(e => e.className || e.tagName).slice(0, 5)};
      })()`);
      assert.equal(v.scroll, v.inner, `${width}px: no sideways scroll`);
      assert.deepEqual(v.wide, [], `${width}px: nothing overflows the viewport`);
      assert.ok(Math.abs(v.frame[0] - v.stage[0]) < 1 && Math.abs(v.frame[1] - v.stage[1]) < 1, `${width}px: the viewer fills its frame ${v.frame} ${v.stage}`);
    } finally { await b.close(); }
  }
  const b = await open(); t.after(() => b.close());
  const links = await b.evaluate(`[...document.querySelectorAll('a[href], img[src], link[href]')].map(e => e.getAttribute('href') ?? e.getAttribute('src'))`);
  for (const link of links) {
    if (link.startsWith('https://github.com/ripopov/vtr')) continue;
    if (link.startsWith('#')) {
      assert.ok(link === '#top' ? html.includes('id="top"') : html.includes(`id="${link.slice(1)}"`), `anchor ${link}`);
      continue;
    }
    const url = new URL(link, `http://x${PAGE}`);
    await access(join(root, decodeURIComponent(url.pathname))).catch(() => assert.fail(`missing ${link}`));
  }
  const tips = [];
  for (const i of [1, 2, 3, 4, 0]) {
    await b.evaluate(`document.querySelectorAll('.tabs button')[${i}].click()`);
    tips.push(await b.evaluate(`[document.getElementById('tip').textContent, [...document.querySelectorAll('.tabs button')].map(e => e.getAttribute('aria-selected')).join()]`));
  }
  assert.equal(new Set(tips.map(t => t[0])).size, 5, 'each tab has its own tip');
  assert.deepEqual(tips.map(t => t[1].split(',').indexOf('true')), [1, 2, 3, 4, 0]);
});

test('without the WASM bundle the demo explains how to build it', {timeout: 60000}, async t => {
  const b = await open(); t.after(() => b.close());
  await b.wait(`window.volnaDemo.state === 'missing'`);
  const v = await b.evaluate(`({src: document.getElementById('volna').getAttribute('src'), text: document.getElementById('overlay').textContent,
    visible: getComputedStyle(document.getElementById('overlay')).opacity})`);
  assert.equal(v.src, 'about:blank');
  assert.ok(v.text.includes('volna/volna/web/build.sh'));
  assert.equal(v.visible, '1');
  assert.deepEqual(b.exceptions, []);
});

test('the live demo restores the landing workspace and takes input through the page', {timeout: 180000}, async t => {
  await access(join(root, 'volna/volna/web/dist/volna_bg.wasm')).catch(() => {
    throw new Error('volna/volna/web/dist is missing: run volna/volna/web/build.sh first');
  });
  const b = await open({withViewer: true}); t.after(() => b.close());
  await b.wait(`window.volnaDemo.state !== 'loading'`);
  assert.equal(await b.evaluate('window.volnaDemo.state'), 'ready');
  // The viewer's own state, logged by its debug_state() export inside the frame.
  const state = () => b.evaluate(`new Promise((resolve, reject) => {
    const w = document.getElementById('volna').contentWindow, original = w.console.info;
    const timer = setTimeout(() => { w.console.info = original; reject(new Error('no viewer state')); }, 5000);
    w.console.info = (...args) => { original.apply(w.console, args); const text = args.join(' ');
      if (text.includes('STATE ')) { clearTimeout(timer); w.console.info = original; resolve(text); } };
    w.eval("import('./dist/volna.js')").then(m => m.debug_state());
  })`);
  const ready = await until(async () => {
    const text = await state().catch(() => '');
    return /panel=2 pipeline .* ready rows=420/.test(text) && /panel=3 transaction .* ready/.test(text) && /panel=4 table .* rows=30 /.test(text) ? text : null;
  }, 'the workspace restored with its data', 60000);
  const lines = ready.split('\n');
  const panel = id => lines.find(l => l.includes(`panel=${id} `)) ?? '';
  assert.match(panel(1), /panel=1 focused=false linked=\(true,true\) items=23 loaded=16 .*cursor=Some\(783\) markers=2 viewport=\(717,849\)/);
  assert.match(panel(2), /panel=2 pipeline track=soc\.cpu0\.pipeline focused=true linked=\(true,true\) ready rows=420/);
  assert.match(panel(3), /panel=3 transaction focused=false ready record=Some\(\("soc\.cpu0\.pipeline\.instruction", 245\)\) pinned=true/);
  assert.match(panel(4), /panel=4 table focused=false rows=30 /);

  // A click in the waves moves the cursor there, and M drops a third marker.
  const point = await b.evaluate(`(() => {
    const f = document.getElementById('volna'); f.scrollIntoView({block: 'center', behavior: 'instant'});
    const r = f.getBoundingClientRect(), scale = r.width / f.offsetWidth;
    return {x: r.x + 780 * scale, y: r.y + 300 * scale};
  })()`);
  await b.send('Input.dispatchMouseEvent', {type: 'mouseMoved', ...point});
  await b.send('Input.dispatchMouseEvent', {type: 'mousePressed', button: 'left', clickCount: 1, ...point});
  await b.send('Input.dispatchMouseEvent', {type: 'mouseReleased', button: 'left', clickCount: 1, ...point});
  const clicked = await until(async () => { const s = await state(); return !/cursor=Some\(783\)/.test(s.split('\n').find(l => l.includes('panel=1 '))) ? s : null; }, 'the cursor moved', 10000);
  assert.match(clicked, /panel=1 focused=true/);
  await b.send('Input.dispatchKeyEvent', {type: 'keyDown', key: 'm', code: 'KeyM', text: 'm', windowsVirtualKeyCode: 77});
  await b.send('Input.dispatchKeyEvent', {type: 'keyUp', key: 'm', code: 'KeyM', windowsVirtualKeyCode: 77});
  await until(async () => /panel=1 .*markers=3/.test(await state()), 'a marker at the cursor', 10000);
  assert.deepEqual(b.exceptions, []);
});
