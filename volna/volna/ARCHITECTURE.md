# Architecture

Volna is the official VTR/VDB viewer. It is built as a toolkit-independent core
plus thin frontends, following "Volna direction" in the repository `AGENTS.md`:
every decision the viewer makes lives in `volna-core`; a frontend owns a window,
paints what the core produces, hosts native widgets for the chrome, and runs the
loads the core asks for. Future VDB attachment belongs above the core's session
interface: source semantics, annotations and presentation stay in the separate
VDB layer. VDB attachment is not yet implemented.

```
volna/volna-core      the viewer, no GUI toolkit (builds and tests on every platform)
  src/app.rs             App: Command in, Event out, LoadRequest/LoadResult, layout + render
  src/document.rs        Document: open trace, shared navigation, markers, translators, loads
  src/panels/            stable IDs, split/tab layout, focus, per-panel wave models
  src/workspace/         JSON codec, restore plans, save tickets, preferences and lifecycle
  src/session.rs         Session trait; OpenSpec; batched load requests/results
  src/data/fst_source.rs private fst-reader adapter and mutable reader ownership
  src/data/vtr_source.rs LocalSession over vtr::Reader with shared immutable histories
  src/data/              values, histories, translators, hierarchy
  src/wave/              viewport math, timeline, WaveModel, WaveLayout, painter → Scene
  src/sidebar/           ScopeTreeModel, VariableListModel
  src/scene.rs           Scene display list, FontRole, TextMeasure, TextCache
  src/theme/             Theme<C> tokens, One Dark, host palettes, VS Code snapshot parser
  src/geometry.rs        Point/Rect/Modifiers/CursorIcon in logical pixels
  src/icons.rs, assets/  Lucide SVGs and the bundled fonts, shared by every frontend
  tests/headless.rs      command → state → Scene assertions, no GPU

volna/volna           GPUI frontend: native macOS app, wasm page, VS Code extension
  src/app.rs             Workspace: chrome in GPUI, dispatches Commands, runs loads
  src/dock.rs            stock DockArea adapter over the core split/tab tree
  src/native_workspace.rs atomic file I/O, paths, preferences and native options
  src/wave/table.rs      WaveTable element: hitboxes from the layout, paints the Scene
  src/sidebar/           uniform_list rows over the core models
  src/theme.rs           core theme mapped to Hsla once per install
  src/ui/                button styling, popup placement, text input, splitter, icons, headers
  web/, vscode-ext/      wasm bundle and the extension host bridge
  tests/viewer.rs        macOS Metal harness: real input events, screenshots, frame times

volna/volna-egui      egui/eframe frontend: native desktop only
  src/lib.rs             VolnaApp: egui panels for the chrome, wave canvas, input routing
  src/paint.rs           Scene → egui::Painter, text measurement, stroked icons
  src/headless.rs        drives frames without a window; software rasterizer for tests
  tests/screenshots.rs   interaction sequence with PNG output under results/egui/
```

## The core contract

A frontend talks to [`App`](../volna-core/src/app.rs) through four surfaces.

**Commands in.** Everything the user does is a `Command`: `Open(OpenSpec)`,
`Action(Action)` for the keyboard actions, `Pointer(PanelId, PointerEvent)` for the wave
panel, scope and variable list operations, filter text, menu choices, sash drags.
Frontends translate their own events (GPUI actions, egui keys) into these; they
never mutate viewer state directly.

**Events out.** `take_events()` returns what the core wants done: `Changed`
(repaint), `OpenFileDialog`, `RevealScopeRow`/`RevealVarRow` (scroll a list),
`FocusFilter`, `LayoutChanged { revision }`, `Notice`, and workspace I/O/dialog
requests. Workspace writes carry a `SaveTicket` and opaque JSON bytes; hosts
acknowledge the ticket through `workspace_saved`. Repaint events coalesce
until drained. Dock proposals carry the layout revision they were based on.

**Loads, pull based.** Opening a trace or adding a signal queues a
`LoadRequest`; `take_requests()` hands them to the frontend, which performs them
on whatever executor it has (GPUI's background executor, a thread in egui, the
browser's main loop on wasm) and returns the `LoadResult` through `deliver()`.
Requests carry the document generation they were made under; a newer open,
close or session replacement makes late results no-ops. Tests exercise this
ownership model with a plain loop that performs requests in any order.

**Layout and paint.** Each frame the frontend calls `layout_waves(panel, bounds,
theme)` and then `render_waves(panel, theme, measure)` (or `render_waves_into` with its
own buffer). The layout gives the frontend the rectangles it needs for hit
regions; the `Scene` is a flat display list of `Quad`, `Lines`, `Text`, `Icon`
and `PushClip`/`PopClip` primitives with resolved colours and font *roles*, plus
the pointer shapes for hover regions. Text widths come back through the
`TextMeasure` trait, cached per string in the core so a label is shaped once.

Animations use an explicit clock: `tick(now)` advances the viewport animation
and reports whether another frame is needed, so tests are deterministic and a
frontend only requests frames while something moves.

## Rendering cost is O(pixels), not O(transitions)

The painter in `volna-core/src/wave/paint.rs` never iterates a signal's
changes. For each pixel column it asks the history for the index of the last
change at the column's right edge. `SignalHistory::index_at_hint` performs an
exponential ("galloping") search from the previous column's index, so a sweep
across `W` columns costs `O(W · log(changes per column))`. A column with no
change extends the current run; one change draws an edge; two or more changes
collapse into a "dense" column drawn as a filled band. Bus rows are built the
same way into segments, then each wide-enough segment shapes and paints its
translated value once.

Consequences:

- the ignored frame-time harness measures scaling from 10 K to 100 M
  transitions for a fixed viewport (see `VERIFICATION.md`), and
- a history only needs `len`, `time(i)`, `value(i)`; VTR's `SignalData` gives
  this directly from its shared immutable buffers, and the synthetic source
  computes it procedurally.

Text runs and 1 px lines are snapped to whole logical pixels, so they are crisp
at 1× and 2× DPI. The display list uses a reused buffer, and the shaped-text
cache avoids reshaping repeated labels across frames.

## Document versus view

`App::panels` owns a toolkit-neutral tree of splits and tab groups, stable
monotonic panel IDs, panel content, and focus. Split clones rows and formats
while retaining shared immutable histories; a new tab starts empty. Close
collapses empty groups and one-child splits, preserving surviving size ratios.
The last waveform panel is emptied rather than removed. Layout proposals are
validated atomically for membership, duplicates, active tabs, finite positive
shares, depth and panel count. `debug_state()` reports one line per panel in
layout order, with its ID, focus and link flags before the waveform state.

`Document` owns the session, shared viewport and cursor, markers with stable IDs
and optional labels, translators, and load generations. Each `WaveModel` owns
its rows, selection, columns, scroll and transient input. Its two link flags
choose between document navigation and local navigation through effective
accessors. Linked viewports share one animation; `App::tick` advances it once,
plus every independent animation. Unlink snapshots the displayed shared value;
relink adopts the retained shared position even if no panels currently follow
it. Markers always use the focused panel's effective cursor.

Pointer commands name a panel; keyboard actions and sidebar additions resolve
focus when handled. Deliveries fan out shared history Arcs to all matching
panels. Adding a loaded signal in another panel reuses the history without
queuing another decode. New panel IDs advance the existing allocator. Restoring saved IDs bumps the
document generation; frontend callbacks carry that generation, so delayed
pointer and menu events cannot target a replacement panel.

`Hierarchy::find_scope` and `find_var` resolve literal path segments and return
`Found`, `Missing`, or `Ambiguous`. `var_path` includes an optional declaration
occurrence for duplicate variable names. Duplicate scopes remain ambiguous,
including when a variable occurrence is supplied; names containing dots are
never split. These lookups are metadata operations, separate from trace queries.

The main GPUI frontend hosts every visible panel in the stock dock component.
Its adapter converts the toolkit's resolved drag/drop tree to core IDs and
normalized fractions; pixel sizes and the toolkit's serialized state never enter
workspace files. The same dock path hides the tab header for a single panel.
The core owns focus outlines and inactive cursor colours. egui retains its
existing single-panel feature set and has persistence disabled.

## Workspace persistence

`workspace::Workspace` is versioned JSON, separate from VTR, VDB and the raw
query protocol. It captures panel rows, formats, links, navigation, markers,
layout and sidebar state. `prepare` validates the entire file and resolves
hierarchy paths without mutating the app; `RestorePlan::commit` replaces the
view and invalidates older history loads. Unknown panels retain their raw JSON.
Unresolved rows/scopes and unavailable translator names survive subsequent saves.
Integer cursor/marker times remain `u64`; hosts never parse workspace JSON.

`workspace::State` owns resource identity and the save lifecycle. The scheduler
tracks revision, epoch and destination, allows one outstanding write, and uses
`tick(now)` for one-second idle saves. Input comparisons exclude hover and
animation frames and do not serialize signal lists on pointer motion. Explicit
Open prepares first, flushes the old state, then commits. Save As switches the
active destination only on acknowledgement; failed flushes keep the live view.
Automatic restore failures suspend saves. Fallback selection compares the
saved `supersedes` SHA-256 against the exact sidecar bytes and preserves its base.

`App::new()` disables persistence. Native `main` configures a byte store that
reads bounded files and writes through a same-directory temporary file and
rename. The WASM entry point accepts both opaque restore candidates with the
trace, then passes core save events to the host. VS Code uses `workspace.fs`
for files and `workspaceState` for fallback storage; ticket counters cross
JavaScript as decimal strings. Its custom editor disables multiple editors per
document, requests a snapshot on hide and flushes the last received snapshot
on disposal. Native quit and trace transitions flush before replacing state.

Preferences live outside workspace files. Native stores them under the config
directory; VS Code supplies settings and theme snapshots. Tests and the egui
frontend do not opt into workspace storage.

## The session seam

All trace data is reached through the `Session` trait: resident `info()` and
`hierarchy()`, and expensive `load_signal()`/`load_signals()` queries.
`LocalSession` derives event shapes from `Reader::signal_var_type`, using the
VTR hierarchy's existing declaration index. It keeps no separate event registry;
variable rows and loaded histories follow the same declaration.
`LocalSession` implements it over `vtr::Reader`, memory-mapped from a path
natively and parsed from an in-memory image on wasm. The private FST adapter
owns a buffered file or byte cursor and serializes mutable fst-reader access
inside the backend. `SynthSource` supplies the procedural stress trace.
`OpenSpec::open` detects the format from the image header.

The document coalesces queued signal loads into `LoadRequest::Signals`; FST
uses one filtered read for the batch. Results identify each signal and carry
per-signal success or failure under one document generation. FST aliases and
repeated requested identities share an immutable history. Frontends execute
requests without knowing the reader's locking or storage types. Native loads
run off the UI thread; the current wasm executor is single-threaded and can
block while decoding. See the [FST support notes](README.md) for extended
metadata limitations and explicit unsupported cases.

`Session::capabilities()` reports operations, independently of recorded content.
`transactions()` and `relations()` return optional query facets. VTR provides
both even for empty recordings; FST and the synthetic waveform source provide
neither. Thus `None` means unsupported, while an empty visit, a missing record
or an empty relation list is a supported query returning no data. Facets avoid
requiring waveform-only backends to implement dummy transaction operations.

Transaction tracks are resident raw metadata with session-local `TrackRef`
identities and resolved paths, stream kinds and attributes. Expensive queries
visit transactions by generator, stream and inclusive overlap window, fetch a
`TransactionRef`, or obtain incoming/outgoing relations. VTR's attribute
phases, typed values, parent, status, kind, events and stages survive this
adapter. Strings are resolved at the boundary, so query results outlive the
session without backend string-table handles. The small semantic status/kind/
phase enums are shared with VTR; no reader or buffer types cross the boundary.
The visitor stops on false and does not promise chronological order. These
facets establish future view extension points; no transaction views or load
requests are issued by today's wave-only frontends. Callers must execute these
blocking queries away from native UI frames. Relation vectors and decoded
transaction block caches are not memory-bounded remote query results.

### Future bounded queries

Full histories are the current implemented waveform query. They do not meet
the remote-file objective: an expensive history still transfers and retains
all changes for the selected signal. A remote transport must wait for bounded
waveform-window and summary queries; no protocol or remote server exists.

Extend the same Session/load-request seam with explicit query identities and
window results, keeping resident metadata separate. A window contract must
define a half-open `[start, end)` interval, the value immediately before
`start`, every change at `start` in source order, and exclusion of changes at
`end`. It must distinguish unknown boundary data from a known X value, empty
complete intervals from unavailable data, and complete results from truncated
ones. A caller-specified change/byte limit and a continuation that preserves
same-timestamp ordering are needed; never report a clipped result as complete.
Summary queries must specify bins, transition counts and boundary values and
must label summaries as aggregates rather than exact transitions. Carry
generation plus query identity through completions so a stale pan/zoom request
cannot replace a newer window in the same document. These are requirements for
the future extension, not implemented window behavior or measured bounds.

## Frontends

The GPUI crate keeps the native app, the wasm page and the VS Code extension.
`Workspace` owns one `App` and is the only GPUI entity that holds viewer state.
It renders the chrome with GPUI elements and GPUI Kit's `gpui-component`
controls, registers GPUI actions that dispatch core `Action`s, keeps the popup
menu and filter input in step with the core's menu and filter, and spawns each
`LoadRequest` on the background executor. The window hosts `Workspace` inside
the component `Root` for overlays and focus routing. Buttons, tooltips and menus
use library interactions; `src/ui` adds Volna styling/placement and retains the
custom filter input, splitter, icons and headers. The filter stays custom on
both targets because the component input's focus-loss path in `gpui-pre-web`
0.3.4 blurs the browser's keyboard receiver; see the [rationale](../../docs/RATIONALE.md#volna-viewer).
`WaveTable` is a custom element: `prepaint` asks the core for the layout and
inserts hitboxes for the dividers, badges and marker chips; `paint` builds the
`Scene` with GPUI's text system as the measurer, walks it into `paint_quad`,
`paint_path`, shaped lines and SVGs, and forwards mouse events as
`PointerEvent`s. Nested `PushClip` becomes nested content masks.

The egui crate is native only. `VolnaApp` lays the chrome out with egui panels,
paints rows of the scope tree and variable list itself (icons are stroked, so no
font has to carry glyphs), and draws the wave `Scene` with `egui::Painter`. Its
`headless` module runs frames through `Context::run_ui` and rasterizes the
tessellated meshes in software, which is how its screenshots and interaction
tests run without a window or GPU.

What a frontend owns: the window and event loop, painting, text shaping, native
widgets for trees, lists, menus, tooltips, text input and dialogs, the theme
colour type, and the executor. What it never owns: what a pixel column shows,
what a key does, which loads to issue.

## Loading ownership

All opens use one completion path in the core. The document generation changes
on open, close, or explicit session replacement; late successes and failures are
ignored. The wave model coalesces pending requests by `SignalRef` through the
document and shares loaded `Arc` histories across duplicate/alias rows, while
retaining each variable's name and translator. Rows own loaded histories;
removing the last row releases the viewer's ownership. There is no permanent
history cache. A pending load may finish after removal and can serve a re-added
row in the same generation. Failed loads can be retried by adding the signal
again, updating all its rows together.

## Theming

`volna_core::theme::Theme<C>` is the resolved design-token set: surfaces,
interaction states, waveform strokes, marker chips, fonts and metrics. All
resolution runs on the core `Color` type: `Theme::one_dark()` for native and
standalone web, `Theme::from_host(&HostPalette)` for embedded hosts. A frontend
maps the palette into its own colour type once with `theme.map(...)` and keeps
reading the same field names; the painter emits resolved colours into the
`Scene`, so frontends convert but never choose colours. Metrics are logical
pixels.

The GPUI adapter also projects these tokens into `gpui-component::Theme` and
synchronizes its base theme on installation. Button surfaces receive the
corresponding core colours at their call sites.
Shared waveform rendering and the minimal egui frontend have no component-library
dependency.

Host palettes: `theme/vscode.rs` owns every VS Code colour ID, fallback chain
and appearance inference from the webview's resolved `--vscode-*` snapshot;
`theme/color.rs` parses CSS colours; `HostPalette::from_json` accepts the
host-neutral JSON documented by [the palette example](../volna-core/tests/fixtures/custom-palette.json).
Supplied foreground/background pairs retain their colours; missing or invisible
ones fall back to readable text; thin strokes and chips get a 3:1 contrast floor
by changing lightness only; high-contrast modes keep selection outlines and
stronger borders. Raw snapshots from real VS Code webviews feed this path in
unit and visual tests. The VS Code extension (`vscode-ext/theme.mjs`) only
captures snapshots and delivers them; theme changes replace the GPUI global and
refresh windows, preserving the trace, rows and interaction state.

## Extension points

| Add a… | Do this |
|---|---|
| value translator | implement `data::Translator` in `volna-core`, register it in `Translators::builtin` (or at run time with `register`) |
| signal type | add a `SignalShape` variant, teach `LocalSession::shape_of` to produce it, add a `paint_*_row` branch in `wave/paint.rs` |
| trace source | implement `session::Session` (info + hierarchy + `load_signal`) and an `OpenSpec` variant, or call `App::set_session` |
| view | a model in `volna-core` with its own commands and a `render` into `Scene`; each frontend adds a canvas that paints it |
| chrome panel | render it in each frontend's chrome from core state; keep the decisions in the core |
| colour or metric | add a token to `theme::Theme`; the painter and frontends only read tokens |
| icon | drop the Lucide SVG into `volna-core/assets/icons`, add the variant in `icons.rs`; the egui frontend strokes it in `paint.rs` |
| key binding | GPUI: a `KeyBinding` in `volna/src/app.rs`; egui: `wave_action` in `volna-egui/src/lib.rs`; both map to `core::Action` |
| frontend | paint the `Scene`, implement `TextMeasure`, translate input to `Command`s, drain events, run `LoadRequest`s |

## Platform notes

- Fonts: IBM Plex Sans (UI) and Lilex (mono) are embedded in `volna-core` and
  used by every frontend, so text widths and looks agree across them.
- Components: pin the `gpui-kit` umbrella to 0.6.1 and use its runtime,
  component and asset modules. Its web dependencies compile threading support,
  so the workspace pins a nightly Rust toolchain.
  Required component icons are embedded with `icon_assets!` on both native and
  web; assets do not require a CDN or network access.
- Web: select `gpui_kit::platform::single_threaded_web()` and compile ordinary
  unshared WASM memory; do not enable atomics/shared-memory linker flags or
  rebuild a threaded standard library. Compiled-in threading support does not
  start workers or require `SharedArrayBuffer`. Default VS Code needs no
  cross-origin isolation or extra startup flags. Files arrive as bytes over
  `postMessage` and open through `OpenSpec::Bytes`. `debug_state()` on the wasm
  module logs the core state to the console for browser-driven verification.
- Native: `OpenSpec::Path` memory-maps the file; signal histories are loaded on
  demand off the UI thread when a variable is added.
- egui: the workspace builds `volna-egui` with `opt-level = 2` even in dev, so
  its software-rasterized tests run in seconds.
