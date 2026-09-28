// Shared helpers for the design-system guardrail tests.
import {readdir, readFile} from 'node:fs/promises';
import {dirname, join, normalize, relative, sep} from 'node:path';
import {fileURLToPath} from 'node:url';
import {browserTest} from '../../volna/volna/tools/browser-test.mjs';

export const root = fileURLToPath(new URL('../..', import.meta.url));
export const ds = join(root, 'docs/design-system');
const styles = join(ds, 'styles.css');
const componentsCss = await readFile(join(ds, 'components/components.css'), 'utf8');
/** Custom properties that components read as parameters (for example --icon) rather than tokens. */
const tokenCss = (await Promise.all((await readdir(join(ds, 'tokens'))).map(f => readFile(join(ds, 'tokens', f), 'utf8')))).join('\n');
const TOKENS = new Set([...tokenCss.matchAll(/(--[\w-]+)\s*:/g)].map(m => m[1]));
const PARAMETERS = new Set([...componentsCss.matchAll(/var\((--[\w-]+)/g)].map(m => m[1]).filter(t => !TOKENS.has(t)));
const TYPES = {'.html': 'text/html', '.css': 'text/css', '.js': 'text/javascript', '.mjs': 'text/javascript',
  '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2', '.ttf': 'font/ttf'};
/** Chrome flags that keep text rasterisation stable across machines. */
export const STABLE_RENDERING = ['--font-render-hinting=none', '--force-color-profile=srgb', '--disable-lcd-text',
  '--hide-scrollbars'];

export async function files(dir) {
  const out = [];
  for (const e of await readdir(dir, {withFileTypes: true})) {
    const path = join(dir, e.name);
    if (e.isDirectory()) out.push(...await files(path));
    else out.push(path);
  }
  return out;
}

/**
 * HTML pages that adopt the design system: every page under docs/ that links
 * docs/design-system/styles.css, except the design system's own frozen
 * reference (component cards, guideline specimens, UI kit, thumbnail).
 */
export async function adoptingPages() {
  const pages = [];
  for (const file of await files(join(root, 'docs'))) {
    if (!file.endsWith('.html')) continue;
    const inside = relative(ds, file);
    if (!inside.startsWith('..') && inside !== 'gallery.html') continue;
    const html = await readFile(file, 'utf8');
    const links = [...html.matchAll(/<link\b[^>]*rel=["']?stylesheet[^>]*>/gi)]
      .map(([tag]) => tag.match(/href=["']([^"']+)["']/i)?.[1]).filter(Boolean);
    if (links.some(href => !/^[a-z]+:/i.test(href) && normalize(join(dirname(file), href)) === styles)) pages.push(file);
  }
  return pages.sort();
}

const COLOUR_PROPS = /^(color|background(-color)?|border(-(top|right|bottom|left|block|inline))?(-color)?|outline(-color)?|fill|stroke|box-shadow|text-shadow|text-decoration(-color)?|caret-color|accent-color|column-rule(-color)?)$/;
const COLOUR_LITERAL = /#[0-9a-f]{3,8}\b|\b(rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(/i;
const NAMED_COLOUR = /(^|[\s,(])(white|black|gr[ae]y|silver|red|maroon|orange|yellow|olive|lime|green|teal|aqua|cyan|blue|navy|fuchsia|magenta|purple|pink|brown|gold)(?=$|[\s,)])/i;

/** Declarations of a style attribute, or of every rule body in a stylesheet. */
function declarations(css, {sheet}) {
  css = css.replace(/\/\*[\s\S]*?\*\//g, '');
  if (sheet) css = [...css.matchAll(/\{([^{}]*)\}/g)].map(m => m[1]).join(';');
  return [...css.matchAll(/(?:^|[;\s])(--?[\w-]+|[a-z-]+)\s*:\s*([^;]+)/gi)]
    .map(([, prop, value]) => [prop.toLowerCase(), value.trim()]);
}

/** Design-system violations in one declaration block (a page's CSS or a maintained stylesheet). */
function lintDeclarations(css, {page, sheet = true}) {
  const problems = [];
  for (const [prop, value] of declarations(css, {sheet})) {
    if (page && prop.startsWith('--') && !PARAMETERS.has(prop)) problems.push(`defines custom property ${prop}; add tokens to the design system`);
    const bare = value.replace(/var\([^()]*(\([^()]*\))*[^()]*\)/g, 'var()').replace(/url\([^)]*\)/g, 'url()');
    if (COLOUR_LITERAL.test(bare)) problems.push(`${prop}: ${value} uses a colour literal; use a token`);
    else if (COLOUR_PROPS.test(prop) && NAMED_COLOUR.test(bare)) problems.push(`${prop}: ${value} uses a named colour; use a token`);
    if (prop === 'font-family' && !/^(var\(--font-[\w-]+\)|inherit)$/.test(value)) problems.push(`font-family: ${value}; use var(--font-sans|mono|display)`);
    if (prop === 'font' && !/var\(--font-[\w-]+\)|^inherit$/.test(value)) problems.push(`font: ${value} names no font token`);
    if (!page) continue;
    if (/^border(-[a-z]+)*-radius$/.test(prop) && !/^((var\(\)|0|50%|inherit)(\s+(var\(\)|0|50%))*|calc\(.*var\(\).*\))$/.test(bare))
      problems.push(`${prop}: ${value}; use var(--radius-*)`);
    if (prop === 'box-shadow' && !/^(none|var\(\)(\s*,\s*var\(\))*)$/.test(bare)) problems.push(`box-shadow: ${value}; use var(--shadow-*)`);
  }
  return problems;
}

/** Violations of the page rules in AGENTS.md "Web pages and design system". */
export function lintPage(html) {
  const problems = [];
  for (const [, css] of html.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/gi)) {
    if (/@font-face/i.test(css)) problems.push('defines @font-face; fonts come from the design system');
    if (/@import/i.test(css)) problems.push('uses @import; link only the design system');
    problems.push(...lintDeclarations(css, {page: true}));
  }
  for (const [, , css] of html.matchAll(/\sstyle=(["'])([\s\S]*?)\1/gi)) problems.push(...lintDeclarations(css, {page: true, sheet: false}));
  for (const [tag] of html.matchAll(/<link\b[^>]*rel=["']?stylesheet[^>]*>/gi))
    if (!/href=["'][^"']*design-system\/styles\.css["']|href=["']styles\.css["']/i.test(tag)) problems.push(`links another stylesheet: ${tag}`);
  return problems;
}

/** Violations in a maintained design-system stylesheet: colours and fonts come from tokens. */
export function lintStylesheet(css) {
  return lintDeclarations(css, {page: false});
}

function repository() {
  return new Proxy({}, {get(_, path) {
    if (typeof path !== 'string') return undefined;
    if (path === '/') return {type: 'text/html', body: '<!doctype html><title>root</title>'};
    const file = normalize(join(root, decodeURIComponent(path)));
    if (!file.startsWith(root) || file.endsWith(sep)) return undefined;
    return {type: TYPES[file.slice(file.lastIndexOf('.'))] ?? 'application/octet-stream', body: () => readFile(file)};
  }});
}

/** A headless browser over the repository with stable rendering. */
export async function browser() {
  const b = await browserTest(repository(), {ready: `location.protocol === 'http:'`, args: STABLE_RENDERING});
  const origin = await b.evaluate('location.origin');
  /** Navigate to a repository file and wait until its fonts and images are ready. */
  b.open = async (file, {width = 1280, height = 800, reducedMotion = false} = {}) => {
    await b.send('Emulation.setDeviceMetricsOverride', {width, height, deviceScaleFactor: 1, mobile: false});
    await b.send('Emulation.setEmulatedMedia', {features: [{name: 'prefers-reduced-motion', value: reducedMotion ? 'reduce' : 'no-preference'}]});
    const path = '/' + relative(root, file).split(sep).join('/');
    await b.send('Page.navigate', {url: origin + path});
    await b.wait(`document.readyState === 'complete' && location.pathname === ${JSON.stringify(path)} && (!document.querySelector('[data-gallery]') || window.ready === true)`);
    await b.evaluate(`Promise.all([document.fonts.ready, ...[...document.images].map(i => i.decode().catch(() => {}))])
      .then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))).then(() => true)`);
  };
  return b;
}

/**
 * In-page contrast audit: every rendered text run against its composited
 * background, and the foundation token pairs, in the current theme.
 * WCAG 2.2: 4.5:1 for text, 3:1 for large text and non-text marks.
 */
export const CONTRAST = String.raw`(() => {
  const parse = value => {
    let m = value.match(/^rgba?\(([\d.]+),\s*([\d.]+),\s*([\d.]+)(?:,\s*([\d.]+))?\)$/);
    if (m) return [+m[1], +m[2], +m[3], m[4] === undefined ? 1 : +m[4]];
    m = value.match(/^color\(srgb ([\d.e-]+) ([\d.e-]+) ([\d.e-]+)(?: \/ ([\d.e-]+))?\)$/);
    if (m) return [m[1] * 255, m[2] * 255, m[3] * 255, m[4] === undefined ? 1 : +m[4]];
    throw new Error('unparsed colour ' + value);
  };
  const over = (top, below) => {
    const a = top[3] + below[3] * (1 - top[3]);
    return a === 0 ? [0, 0, 0, 0] : [0, 1, 2].map(i => (top[i] * top[3] + below[i] * below[3] * (1 - top[3])) / a).concat(a);
  };
  const lum = c => { const l = c.slice(0, 3).map(v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2]; };
  const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
  const hex = c => '#' + c.slice(0, 3).map(v => Math.round(v).toString(16).padStart(2, '0')).join('');
  const background = el => {
    const layers = [];
    for (let e = el; e; e = e.parentElement) {
      const s = getComputedStyle(e);
      if (s.backgroundImage !== 'none' && !/gradient/.test(s.backgroundImage)) return null;
      const c = parse(s.backgroundColor);
      if (c[3] > 0) layers.push(c);
      if (c[3] === 1) break;
    }
    return layers.reduceRight((acc, c) => over(c, acc), [255, 255, 255, 1]);
  };
  const name = el => el.tagName.toLowerCase() + [...el.classList].map(c => '.' + c).join('') ;
  const failures = [];
  for (const el of document.body.querySelectorAll('*')) {
    const text = [...el.childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join('').trim();
    if (!text || !el.getClientRects().length || el.closest('.v-visually-hidden,[disabled],[aria-disabled="true"],template,script,style')) continue;
    const s = getComputedStyle(el);
    if (s.visibility !== 'visible') continue;
    let alpha = 1;
    for (let e = el; e; e = e.parentElement) alpha *= +getComputedStyle(e).opacity;
    const bg = background(el);
    if (!bg) continue;
    const fg = parse(s.color); fg[3] *= alpha;
    const size = parseFloat(s.fontSize), large = size >= 24 || (size >= 18.66 && +s.fontWeight >= 700);
    const need = large ? 3 : 4.5, r = ratio(over(fg, bg), bg);
    const theme = el.closest('[data-theme]').dataset.theme;
    if (r < need - 0.005) failures.push(theme + ' ' + name(el) + ' "' + text.slice(0, 32) + '" ' + hex(over(fg, bg)) + ' on ' + hex(bg) + ' = ' + r.toFixed(2) + ' < ' + need);
  }
  const TEXT = [...['text-1', 'text-2', 'text-3'].flatMap(f => ['bg', 'bg-raised', 'surface-card', 'code-bg'].map(b => [f, b])),
    ...['bg', 'surface-card', 'accent-soft'].map(b => ['accent-text', b]), ['on-accent', 'accent'], ['on-accent', 'accent-hover'],
    ['warning', 'warning-soft'], ['danger', 'danger-soft'], ['cmp-theirs-text', 'cmp-theirs'],
    ...['code-text', 'code-comment', 'code-keyword', 'code-string', 'code-number', 'code-fn', 'code-prompt'].map(f => [f, 'code-bg'])];
  const MARKS = [...['wave-signal', 'wave-undef', 'wave-highimp', 'wave-dontcare', 'wave-weak', 'wave-event', 'wave-bus-text', 'wave-cursor',
    'tx-in', 'tx-out', 'marker-1', 'marker-2', 'marker-3', 'marker-4', 'marker-5', 'marker-6'].map(f => [f, 'wave-bg']),
    ['focus-ring', 'bg'], ['focus-ring', 'surface-card'], ['accent', 'bg'],
    ...['success', 'warning', 'danger', 'info'].map(t => [t, t + '-soft'])];
  const probe = document.createElement('span');
  document.body.append(probe);
  const token = t => { probe.style.color = 'var(--' + t + ')'; return parse(getComputedStyle(probe).color); };
  const theme = document.documentElement.dataset.theme;
  for (const [pairs, need] of [[TEXT, 4.5], [MARKS, 3]]) for (const [f, b] of pairs) {
    const bg = token(b), r = ratio(over(token(f), bg), bg);
    if (r < need - 0.005) failures.push(theme + ' token --' + f + ' on --' + b + ' = ' + r.toFixed(2) + ' < ' + need);
  }
  probe.remove();
  return failures;
})()`;
