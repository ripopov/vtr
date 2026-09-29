# Volna design system

Design system for every HTML page in this repository: the Volna website (marketing pages, benchmarks, the VTR format overview, docs and downloads) and, as they are revised, the proposal and demo pages under `docs/`.

## Ownership

This directory is the source of truth. It started as a Claude Design export; edit it here, not in Claude Design, and do not re-import over it.

- **Maintained:** `styles.css`, `tokens/`, `components/base.css`, `components/components.css`, `gallery.html`, `baselines/`, `guidelines/` and `fonts/build.sh`. Pages use this layer: link `styles.css`, set `data-theme`, and compose the `v-` classes in static HTML. New tokens and `v-` classes go here, with a guideline card when they add a foundation, instead of into a page's own `<style>`.
- **Frozen reference:** `components/**/*.jsx`, `*.d.ts`, `*.prompt.md`, `_ds_bundle.js` (compiled from the JSX by Claude Design; nothing here rebuilds it), `ui_kits/website/`, and the export metadata `_ds_manifest.json`, `_adherence.oxlintrc.json`, `.thumbnail`, `thumbnail.html` and the `@dsCard` comments. They show the intended pages and component markup; copy their structure into static HTML rather than editing or extending them.
- **Referenced, never copied:** fonts (`volna/volna-core/assets/fonts`), viewer icons (`volna/volna-core/assets/icons`) and the app icon (`volna/volna/assets/app-icon`).
- `gallery.html` renders every `v-` class in both themes. A new class gets a specimen there in the same change.

### Guardrails

All headless, run by `.github/workflows/design-system.yml`:

- `docs/tests/design-system.test.mjs`: every relative reference resolves; the fonts, icons and app icon load.
- `docs/tests/design-system-rules.test.mjs`: pages that link `styles.css` (the adopting pages) contain no colour literals, font families, custom tokens, raw radii or shadows, `@font-face`, or other stylesheets; the maintained stylesheets take colours and fonts from tokens; the gallery covers every `v-` class; every rendered text run and the foundation token pairs meet WCAG AA (4.5:1 text, 3:1 large text and marks) in both themes, and every viewer value stroke and marker reaches 4.5:1 on the viewer canvas.
- `docs/tests/design-system-visual.test.mjs`: the gallery's computed styles and per-section screenshots match `baselines/`; adopting pages fit 360px without horizontal scrolling; every keyboard stop shows the focus ring; reduced motion stops all motion.

After an intended visual change, run `UPDATE_BASELINES=1 node --test --test-concurrency=1 docs/tests/design-system-visual.test.mjs` and review the `baselines/` diff (PNG before/after and `styles.json`) before committing. A failing comparison writes baseline, current and diff images to `DESIGN_DIFF_DIR` (CI uploads them as the `design-diff` artifact). Screenshots hide product images, so only the design is compared.

## Product context

- **Volna** is an open, native waveform and transaction viewer for hardware simulation (RTL, SystemC, gem5). One Rust core (`volna-core`) with a GPUI frontend (Zed's toolkit) that runs natively, in the browser via WASM, and inside VS Code. It opens VTR and FST.
- **VTR** (Volna Trace Record) is an open trace format: one file for waveforms, transactions, hierarchy and simulator logs; smaller and faster than FST on every benchmark in the report.
- **VDB** is the separate design/presentation companion (colours, stage meaning, source locations). VTR never stores presentation.
- Audience: design and verification engineers, CPU/SoC architects, EDA tool builders. Used to GTKWave, Surfer, Verdi, Konata. They trust numbers, real screenshots and keyboard-driven tools.
- Planned: agent control over MCP.

## Sources

- The direction was derived from these repository files:
  - `vtr/docs/volna-landing.html` — existing landing page: copy, structure, component shapes.
  - `vtr/volna/volna-core/src/theme/mod.rs` — One Dark theme (surfaces, text, wave colours, markers, fonts, 13px UI / 24px rows).
  - `vtr/volna/volna-core/src/pipeline/palette.rs` — pipeline stage ladder.
  - `vtr/volna/volna/src/theme.rs` — component radius 4px.
  - `vtr/docs/volna-mcp.html` — light/dark doc-page palettes (light semantic colours).
  - `vtr/docs/BENCHMARK_RESULTS.md` — every number used.
  - `vtr/docs/SPEC.md`, `vtr/README.md`, `vtr/volna/volna/README.md`, `vtr/tools/README.md` — format and install facts.
  - Assets: `vtr/volna/volna/assets/app-icon/`, `vtr/volna/volna-core/assets/{fonts,icons}`, screenshots from `vtr/volna/volna/benchmarks/` and `vtr/demos/pipeline-viewer/screenshots/`.
- Repo: https://github.com/ripopov/vtr
- References for quality and tone (brief): zed.dev, linear.app, tailscale.com, astral.sh, ghostty.org.

## Design direction: Instrument

The website wears the viewer's own chrome, so the site and the app read as one product.

- Neutrals are One Dark greys from `volna-core` (#1f2228 → #eceef1). The accent is the viewer blue #74ade8, or #2d6cb0 in light. Radii are 3/4/6/8/10 (the viewer's components use 4). Display type is Plex Sans 600. The primary button uses the accent.
- `data-theme` = `dark` | `light`. Both are first-class; dark is the default (the viewer's native appearance). A 3-line inline head script reads `localStorage['volna-theme']` / `prefers-color-scheme` before paint.
- Two other directions were explored and retired: Phosphor (the teal landing page) and Datasheet (paper, indigo, mono display).

Waveform, marker and pipeline-stage colours come straight from the viewer.

## CONTENT FUNDAMENTALS

- **Voice**: precise, calm, engineering-grade. Confidence comes from measurements, not adjectives. No "blazing", "revolutionary", "seamless".
- **Numbers first**: every claim carries a number and names its workload. "243 MiB vs 389 MiB on openC910 CoreMark", "every read query faster than wellen". Ratios as `1.6×`, shares as `63%`, ranges with an en dash `47–98%`. Units: MiB, ms, s, µs.
- **Person**: product as subject ("Volna shows…", "VTR packs…"); "you/your" for the reader ("your traces", "a trace you already have"). "We" only for the benchmark ("on every benchmark we run").
- **Casing**: sentence case for headings and buttons ("Try the live demo", "View on GitHub"). Mono kickers in caps ("THE FORMAT"). Product names as written: Volna, VTR, VDB, FST, FTR, GTKWave, Surfer, Verilator, VS Code, WebAssembly.
- **Headlines**: short, parallel, often two beats: "Two formats. One modern file." "One recording. Every view of it." "Level up your traces."
- **Body**: say what it does and how. "Painting walks pixel columns, not transitions, so its cost follows the window width." Mention the key to press: "press A to plot it".
- **Honesty about status**: planned features are labelled planned ("Agent control over MCP (planned)"); limits stated plainly ("It needs WebGPU or WebGL2").
- **Fine print** in a single line joined by " · ".
- **No emoji. No exclamation marks.** Unicode arrows are fine in links and captions (→ ↗ ↓).

## VISUAL FOUNDATIONS

- **Colour**: neutral-dominant. One accent (viewer blue), used for the primary action, links, the kicker, the VTR series in charts and selection. Semantic states from One Dark (dark) and GitHub-light (light). A categorical palette for waveform value kinds (signal, X, Z, don't-care, weak, events, cursor, relations) comes straight from `volna-core`.
- **Type**: IBM Plex Sans 400/600 for UI and prose; Lilex 400 for code, signal names, kickers, table numbers and all figures (tabular). Display 40–76px fluid, −0.03em; body 16/1.55; captions 13.
- **Imagery**: real product only — screenshots from the repo and the live WASM viewer in an iframe. No stock photos, no illustrations, no abstract blobs. Screens keep the viewer's own One Dark or light theme; cool, neutral, no grain.
- **Backgrounds**: flat. No gradients except the hatch that marks a saving in comparison bars. (The old landing page's radial glows and gradient headline are retired.)
- **Layout**: 1180px wrap, 1440px for the demo, 24px gutter (16px on phones), 112px section rhythm. Sticky 60px nav. Responsive to 360px; tables scroll inside their frame, never the page.
- **Cards**: 1px border (--border), surface-card fill, radius-lg, no shadow in dark; a 1–2px soft shadow in light only where elevation is real (popovers, the demo frame). Current/selected cards get an accent border, not a glow.
- **Borders**: 1px everywhere; strong borders for controls; kbd uses a 2px bottom border.
- **Shadows**: `--shadow-1` (hairline), `--shadow-2` (menus), `--shadow-frame` (demo window). No coloured glows.
- **Radii**: 3/4/6/8/10px (xs–xl); pills 999px.
- **Transparency & blur**: only the sticky nav (84% bg + 12px backdrop blur).
- **Hover**: surfaces step one neutral up; borders brighten to text-3; links go from accent to text-1. **Press**: selected surface, no scale. **Focus**: 2px accent outline, 2px offset, always visible with :focus-visible.
- **Motion**: 120/150/350ms, ease-out, colour/opacity/width only. No bounce, no parallax, no scroll reveals. All durations 0 under prefers-reduced-motion.
- **Data viz**: VTR in the accent, the other format in a neutral; bars to scale with a hatched "saved" ghost. Numbers right-aligned, mono, tabular.

## ICONOGRAPHY

- The viewer uses **Lucide** (ISC licence, `volna/volna-core/assets/icons/LICENSE`), 24px grid, 1.5–2px stroke, rendered at 16px. The viewer's own SVGs live in `volna/volna-core/assets/icons/`; the site references them there rather than copying them.
- On the site, the `Icon` component loads Lucide from CDN (`lucide-static@0.469.0`) as a CSS mask so it takes `currentColor`. Icons the viewer set lacks (copy, check, download, github, sun, moon, monitor, laptop, globe, square-code, menu) come from the same Lucide set — no substitution of style.
- Icons are functional: nav, buttons, feature cards, callouts. Never decorative clusters.
- No emoji. Unicode arrows (→ ↗ ↓) and × are used as typographic glyphs in links, ratios and captions.
- Logo: `volna/volna/assets/app-icon/volna.svg` (seal on a wave, canonical app icon) and `volna-mark.svg` (single-colour mark). Pair the icon with the word "Volna" in Plex Sans 600. Never recolour the app icon.

## Index

- `styles.css` — entry point (imports only).
- `tokens/` — fonts, palette (scales), theme (semantic roles), viewer (the Volna viewer theme's canvas, value, marker and stage tokens, proposed in `docs/volna-theme.html`), typography, spacing, elevation, motion.
- `components/base.css`, `components/components.css` — element defaults and `v-` classes (usable in static HTML without React).
- `components/<group>/` — React components with `.d.ts` and `.prompt.md`, one card per group.
- `guidelines/` — foundation specimen cards (Colors, Type, Spacing, Brand).
- `ui_kits/website/` — click-through site: Landing, Benchmarks, VTR format, Docs, Download.
- `assets/screens/` — product screenshots. Logo and icons are not copied: they are referenced from `volna/volna/assets/app-icon/` and `volna/volna-core/assets/icons/`.
- `fonts/` — WOFF2 built by `fonts/build.sh` from the viewer's TTFs in `volna/volna-core/assets/fonts/` (originals and licences). Plex Sans is converted whole (its OFL reserves the name "Plex"); Lilex is subset to Latin-1 plus the symbols the site uses. `tokens/fonts.css` falls back to the original TTFs.
- `SKILL.md` — agent skill entry.

## Components

- core: **Icon**, **ThemeSwitch**
- actions: **Button**, **TextLink**
- navigation: **TopNav**, **Footer**, **Tabs**
- marketing: **Hero**, **FeatureSection**, **FeatureRow**, **ProofPoints**, **DemoFrame**, **DownloadCard**
- data: **BenchTable**, **ComparisonBars**
- content: **CodeBlock**, **Kbd**, **Callout**, **Chip**
- docs: **DocsLayout**, **DocsSidebar**, **Toc**

### Intentional additions
- **Icon** — wrapper for the Lucide set so icons take currentColor.
- **ThemeSwitch** — light/dark are both first-class; the site needs a control.
- **ProofPoints** — the brief's proof points need a measured-number row.
- **Chip** — file sections, signal names and badges from the existing landing page.
- **DocsSidebar**, **Toc** — the two slots of the requested docs layout.

## Caveats
- No published release exists; download sizes and checksums in the UI kit are placeholders.
- The live WASM demo is not hosted here; DemoFrame shows real screenshots until `DEMO_URL` is set.
