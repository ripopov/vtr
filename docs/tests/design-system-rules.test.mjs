// node --test --test-concurrency=1 docs/tests/design-system-rules.test.mjs
// Guardrails for AGENTS.md "Web pages and design system": pages compose the
// design system instead of restyling it, the gallery shows every class, and
// every rendered text and foundation token pair meets WCAG AA in both themes.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {join, relative} from 'node:path';
import {test} from 'node:test';
import {CONTRAST, adoptingPages, browser, ds, lintPage, lintStylesheet, root} from './design-system-lib.mjs';

const pages = await adoptingPages();
const name = file => relative(root, file);

test('the lint catches each page rule', () => {
  const found = lintPage(`<link rel="stylesheet" href="page.css"><style>@font-face{src:url(a.woff2)}
    .a{color:#fff;background:white;font-family:Arial;border-radius:5px;box-shadow:0 0 3px var(--text-1);--brand:1px}</style>
    <p style="--swatch:rgb(1 2 3)">x</p>`);
  for (const rule of ['@font-face', 'color: #fff', 'background: white', 'font-family: Arial', 'border-radius: 5px',
    'box-shadow: 0 0 3px', '--brand', '--swatch: rgb', 'links another stylesheet'])
    assert.ok(found.some(p => p.includes(rule)), `${rule} not reported in ${JSON.stringify(found, null, 1)}`);
  assert.deepEqual(lintPage(`<link rel="stylesheet" href="design-system/styles.css"><style>.a{color:var(--text-2);border-radius:var(--radius-md);
    font:var(--text-sm) var(--font-mono);box-shadow:var(--shadow-1)}</style><span style="--icon:url(x.svg);--swatch:var(--wave-signal)"></span>`), []);
});

test('pages that adopt the design system use its tokens and classes only', async () => {
  assert.ok(pages.some(p => p.endsWith('gallery.html')), 'the gallery adopts the design system');
  const problems = [];
  for (const page of pages) for (const p of lintPage(await readFile(page, 'utf8'))) problems.push(`${name(page)}: ${p}`);
  assert.deepEqual(problems, []);
});

test('the maintained stylesheets take colours and fonts from tokens', async () => {
  const problems = [];
  for (const sheet of ['components/components.css', 'components/base.css'])
    for (const p of lintStylesheet(await readFile(join(ds, sheet), 'utf8'))) problems.push(`${sheet}: ${p}`);
  assert.deepEqual(problems, []);
});

test('the gallery shows every v- class', async () => {
  const css = (await Promise.all(['components/components.css', 'components/base.css'].map(f => readFile(join(ds, f), 'utf8')))).join('\n');
  const classes = new Set([...css.matchAll(/\.(v-[\w-]+)/g)].map(m => m[1]));
  const gallery = await readFile(join(ds, 'gallery.html'), 'utf8');
  const used = new Set([...gallery.matchAll(/\sclass="([^"]*)"/g)].flatMap(m => m[1].split(/\s+/)));
  assert.ok(classes.size > 100, `${classes.size} classes`);
  assert.deepEqual([...classes].filter(c => !used.has(c)).sort(), []);
});

test('text and foundation tokens meet WCAG AA in both themes', {timeout: 120000}, async t => {
  const b = await browser();
  t.after(() => b.close());
  const failures = [];
  for (const page of pages) {
    // Reduced motion: colours must not be read halfway through a theme transition.
    await b.open(page, {reducedMotion: true});
    for (const theme of ['dark', 'light']) {
      await b.evaluate(`document.documentElement.dataset.theme = '${theme}'`);
      for (const f of await b.evaluate(CONTRAST)) failures.push(`${name(page)}: ${f}`);
    }
  }
  assert.deepEqual([...new Set(failures)], []);
  assert.deepEqual(b.exceptions, []);
});
