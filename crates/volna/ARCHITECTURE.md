# Architecture

Volna is the official VTR/VDB viewer, maintained as a workspace crate. Its current
source adapter displays VTR waveforms. Future VDB attachment belongs above this
runtime-data interface: source semantics, annotations, and presentation remain
in the separate VDB layer. VDB attachment is not yet implemented.

The crate is split so that the parts most likely to grow (signal types,
translators, panels, sources) sit behind small traits and can be added without
touching the rendering core.

```
src/
  data/          toolkit-independent model (no GPUI)
    value.rs       Bit, ValueKind, SignalShape, WaveValue
    history.rs     SignalHistory trait: index_at / index_at_hint / value / bit
    translator.rs  Translator trait + Translators registry (bit, bin, hex, udec, sdec, float, real, text)
    source.rs      WaveSource trait, Hierarchy / Scope / Variable model
    vtr_source.rs  WaveSource over vtr::Reader (mmap on native, from_bytes on wasm)
    synth.rs       procedural stress source (100 M transitions, zero memory)
  theme.rs       semantic colours, contrast/fallback rules, fonts and metrics; a GPUI Global
  assets.rs      embedded Lucide icons and IBM Plex Sans / Lilex fonts
  ui/            primitives: IconButton, TextButton, Splitter, PopupMenu, Scrollbar, TextInput, Tooltip
  sidebar/       ScopeTree and VariableList views (uniform_list, keyboard navigation)
  wave/
    viewport.rs    time window ↔ pixels, zoom about a point, clamping
    timeline.rs    tick placement and SI time formatting
    view.rs        WaveView entity: items, selection, cursor, markers, actions, animation
    table.rs       WaveTable element: paints names/values/waves, header, cursor, markers, scrollbar; mouse input
  app.rs         Workspace root view: title bar, sidebar + splitters, status bar, states, key bindings
  lib.rs         start-up shared by native and web; `web` module = wasm-bindgen bridge
  main.rs        native CLI entry
tests/
  viewer.rs      whole-viewer interaction assertions, optional PNGs, ignored timing measurements
```

`tests/viewer.rs` is a macOS-only Cargo integration harness gated by
`visual-test`. It runs on the main thread for AppKit initialization. It uses
the public workspace diagnostics without exposing private state for tests;
internal regression tests remain alongside their source modules.

## Rendering cost is O(pixels), not O(transitions)

`WaveTable::paint` never iterates a signal's changes. For each pixel column it
asks the history for the index of the last change at the column's right edge.
`SignalHistory::index_at_hint` performs an exponential ("galloping") search
from the previous column's index, so a sweep across `W` columns costs
`O(W · log(changes per column))`. A column with no change extends the current
run; one change draws an edge; two or more changes collapse into a "dense"
column drawn as a filled band. Bus rows are built the same way into segments,
then each wide-enough segment shapes and paints its translated value once.

Consequences:

- frame time is flat from 10 K to 100 M transitions for a given viewport (see
  `VERIFICATION.md`), and
- a history only needs `len`, `time(i)`, `value(i)`; VTR's `SignalData` gives
  this directly from its shared immutable buffers, and the synthetic source
  computes it procedurally.

Text runs and 1 px lines are snapped to whole logical pixels, so they are crisp
at 1× and 2× DPI.

## Extension points

### Loading ownership

All file/demo opens use one completion path in `Workspace`. A generation changes
on open, close, or explicit source replacement; late successes and failures are
ignored. `WaveView` independently advances its signal-load generation on source changes.
It coalesces pending requests by `SignalRef` and shares loaded `Arc` histories
across duplicate/alias rows, while retaining each variable's name and translator.
Rows own loaded histories; removing the last row releases the viewer's ownership.
There is no permanent history cache. A pending load may finish after removal and
can serve a re-added row in the same generation. Failed loads can be retried by
adding the signal again, updating all its rows together.

This deliberately uses existing source/history interfaces and two small generation
counters rather than adding a loader framework. It does not interrupt decoding
already in progress, bound distinct concurrent loads, or move wasm work off the
browser thread. The VTR reader, file format, and decode path are unchanged.

### Adding functionality

| Add a… | Do this |
|---|---|
| value translator | implement `data::Translator`, register it in `Translators::builtin` (or at run time with `register`) |
| signal type | add a `SignalShape` variant, teach `VtrSource::shape_of` to produce it, add a `paint_*_row` branch in `table.rs` |
| trace format | implement `data::WaveSource` (hierarchy + `load_signal`) and call `Workspace::set_source` |
| panel | create an entity with `Render`, add it to `Workspace::render` next to the sidebar or centre |
| colour or metric | add a token to `theme::Theme`; components only read tokens |
| icon | drop the Lucide SVG into `assets/icons`, list it in `assets.rs` and `ui::IconName` |
| key binding | add an action with `actions!` and a `KeyBinding` in `app::init` |

## Platform notes

- Fonts: GPUI's web text system has no system fonts, so IBM Plex Sans (UI) and
  Lilex (mono) are embedded and used on every platform for identical output.
- Web: `gpui_web` single-threaded (`default-features = false` avoids the
  nightly-only `wasm_thread`); the VS Code webview cannot be cross-origin
  isolated, so no `SharedArrayBuffer` is needed. Files arrive as bytes over
  `postMessage` and are parsed with `vtr::Reader::from_bytes`.
- Native: `vtr::Reader::open` memory-maps the file; signal histories are loaded
  on demand on the background executor when a variable is added.

## Host theming

VS Code uses the existing single webview. `vscode-ext/theme.mjs` is the only
VS Code colour-token mapping: it reads the resolved `--vscode-*` CSS variables
from the body and sends semantic RGBA tokens through `web::set_theme`. A
MutationObserver watches root/body style and theme-class changes, so theme
customizations and changes within the same light/dark kind also update. Missing
or malformed tokens are omitted; each snapshot replaces the previous palette,
so values removed by a new theme do not leak from an old one. HC light is checked
before the legacy HC class, which VS Code also attaches to HC light webviews.

Web startup is explicit: `await init()` initializes wasm without creating GPUI;
the VS Code adapter waits for theme metadata and installs the initial palette;
then `start()` launches GPUI. Until launch completes, Rust retains the latest
palette. The launch callback installs it before creating the window and starts
a channel for subsequent themes/file opens. `volnaReady` means file delivery is
safe, and the extension registers its receiver before assigning webview HTML.
The standalone page calls `start()` immediately after wasm initialization and
keeps the existing One Dark default. Live changes never replace webview HTML or
workspace entities: `Theme::install` replaces the GPUI global and refreshes all
windows, including cached child views.

`Theme::from_host` is shared Rust with no VS Code, browser or transport types.
The host supplies optional semantic packed RGBA colours and dark/high-contrast
flags. Rust resolves mode-aware fallbacks, composites translucent surfaces,
keeps readable supplied colours, and adjusts insufficient-contrast colours
toward black or white. Text targets 4.5:1; interactive borders/icons target 3:1
where contrast is enforced. Waves use chart colours, with low-alpha row overlays
and separate cursor/marker label foregrounds. HC adds visible selection outlines
and stronger borders/grid/scrollbars. Panels, inputs, bars and popovers use their
own foreground/background pairs. These are local contrast safeguards, not a
claim of complete accessibility conformance or pairwise colour distinguishability.

This small semantic palette/install boundary can also be used by a future
`embedded_gpui` host. There is no Zed integration, new dependency, or VTR change.

Research: [official webview theming guide](https://code.visualstudio.com/api/extension-guides/webview#theming-webview-content),
[theme colour reference](https://code.visualstudio.com/api/references/theme-color),
[ColorTheme API](https://code.visualstudio.com/api/references/vscode-api#ColorTheme),
[VS Code colour serialization](https://github.com/microsoft/vscode/blob/main/src/vs/workbench/contrib/webview/browser/themeing.ts),
and [VS Code webview style application](https://github.com/microsoft/vscode/blob/main/src/vs/workbench/contrib/webview/browser/pre/index.html).
`activeColorTheme`/`onDidChangeActiveColorTheme` expose a kind, not resolved RGB
values; they are not a substitute for CSS variables. Theme-name lookup and
reading theme extension JSON would miss resolved defaults and user overrides.
An extension-host notification also needlessly races webview style delivery,
so the adapter observes the actual delivered palette instead.
