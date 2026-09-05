# VDB Pipeline Studio

A standalone HTML/CSS/JavaScript UX prototype with **two tabs**: a working pipeline
viewer and a complete Field guide with 12 screenshot-based lessons. Open
[index.html](index.html) directly in a browser. No build or runtime dependencies.

Alternatively, from the repository root:

```sh
python3 -m http.server 8765 --bind 127.0.0.1 --directory demos/pipeline-viewer
```

Open <http://127.0.0.1:8765>. A local server gives saved views a stable browser
origin. With `file://`, storage behavior depends on the browser; the demo still runs.

The viewer includes three deterministic example traces, instruction and stage
selection, map-style two-axis zoom, pan/focus, a synchronized waveform pane, direct dependency links, search and filters,
flush visibility, cycle-range measurement, baseline comparison, bookmarks, pins,
notes, JSON export, a command palette, keyboard navigation, and light/dark themes.
Ctrl/Command + wheel, double-click, and two-finger pinch zoom around the pointer
or gesture; Fit shows all cycles and rows, and Focus returns to detail.
The upper Waveforms pane shows configurable digital, numeric, analog, and instruction-ID
signals: in-flight instructions, ROB/cache occupancy, execution stages, issue/flush
pulses, and supply voltage. Both panes share the cursor, time scale, and measurement
range. Click an execution-stage instruction to select it in the pipeline below; use
Signals + to add or remove traces. In-flight and occupancy signals use per-cycle
bars with a labeled scale fixed across the full dump. Values are synthetic examples of a general viewer.
The tutorial's Try it buttons prepare working examples; screenshots open full size.

Data is synthetic and source mapping illustrative. This is a UI prototype, not a
VTR/VDB file reader or a simulator. All processing and saved state remain local.
No CDN, remote font, analytics, or backend is required. The only external links are
explicit research/documentation links.

## Research

[RESEARCH.md](RESEARCH.md) records the Konata presentation review, comparable tools,
UI/UX guidance, design decisions, and limitations. The repository's trace format and
API implementation are unchanged.

## Browser checks and tutorial screenshots

```sh
cd demos/pipeline-viewer
npm ci
npm test
npm run screenshots
```

Tests use an isolated headless Chrome and start a localhost server automatically.
On macOS, the default executable is `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.
Elsewhere set `CHROME_PATH` to a local Chrome/Chromium executable. The pinned
Playwright dependency is for development only. `npm run screenshots` captures the
17 feature screenshots across the 12 lessons, plus the tutorial overview into `screenshots/`.

The browser suite checks real workflows and generated downloads, including range
validation, persistence, empty states, keyboard/pointer navigation, desktop/narrow
layouts, and tutorial images. It does not constitute a screen-reader audit or a
comparative usability study.

Files: `index.html` provides the shell; `styles.css` contains the design system;
`app.js` implements the example model, controls, and tutorial; `tests/viewer.spec.js`
exercises the UI and captures screenshots. Runtime remains plain HTML and JS.
