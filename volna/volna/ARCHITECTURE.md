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
  src/panels/            stable IDs, split/tab layout, focus, per-panel wave, pipeline and table models
  src/nav/               Tween<T> animation, NavState (links, local viewport/cursor) of every timed panel
  src/pipeline/          RowView row axis, PipelineModel, PipelineLayout, stage palette, painter → Scene
  src/table/             fixed sources/columns, exact row viewport, bounded preparation/details, painter → Scene
  src/workspace/         JSON codec, restore plans, save tickets, state.json and lifecycle
  src/settings/          registry, settings.json store (JSONC, surgical edits, diagnostics), search, schema
  src/session.rs         Session trait; OpenSpec; batched load requests/results
  src/data/fst_source.rs private fst-reader adapter and mutable reader ownership
  src/data/vtr_source.rs LocalSession over vtr::Reader with shared immutable histories
  src/data/              values, histories, translators, hierarchy
  src/wave/              viewport math, timeline, WaveModel, WaveLayout, painter → Scene, shared overlay
  src/sidebar/           ScopeTreeModel, MemberListModel, semantic icons and row descriptions
  src/scene.rs           Scene display list, FontRole, TextMeasure, TextCache
  src/theme/             Theme<C> tokens, One Dark, host palettes, VS Code snapshot parser
  src/geometry.rs        Point/Rect/Modifiers/CursorIcon in logical pixels
  src/icons.rs, assets/  Lucide SVGs and the bundled fonts, shared by every frontend
  tests/headless.rs      command → state → Scene assertions, no GPU

volna/volna           GPUI frontend: native macOS app, wasm page, VS Code extension
  src/app.rs             Workspace: chrome in GPUI, dispatches Commands, runs loads
  src/dock.rs            stock DockArea adapter over the core split/tab tree
  src/native_workspace.rs atomic file I/O, paths, settings/state files, watcher, native options
  src/settings_panel.rs  the Settings tab: gpui-kit pages, search bar, results, item controls
  src/settings_json.rs   the JSON view: Editor with registry completion, hover and diagnostics
  src/palette.rs         the ⌘K command palette over actions and ranked settings
  src/canvas.rs          PanelCanvas element: hitboxes/accessibility from panel layout, paints the Scene
  src/table_panel.rs     GPUI table controls, Go to dialog and details inspector
  src/table_clipboard.*  browser clipboard rejection recovery without table semantics
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

**Layout and paint.** Each frame the frontend calls `layout_panel(panel, bounds,
theme)` and then `render_panel(panel, theme, measure)` (or `render_panel_into` with its
own buffer). The layout (`PanelLayout::Waves`, `::Pipeline` or `::Table`) gives the frontend
the rectangles it needs for hit regions; the `Scene` is a flat display list of
`Quad`, `Lines`, `Text`, `Icon` and `PushClip`/`PopClip` primitives with resolved
colours and font *roles*, plus the pointer shapes for hover regions. Text widths
come back through the `TextMeasure` trait, cached per string in the core so a
label is shaped once. One element paints either kind; a frontend never matches on
what a panel shows.

## Baseline table panel

`PanelKind::Table(TableModel)` presents one generator or an ordered fixed signal
set. `TableSource` persists declaration paths and resolves them through the same
session hierarchy used by Waves and Pipeline. Generator rows borrow the
document's `LoadedGenerator`; signal rows borrow shared `SignalHistory` owners.
Only a multi-signal table owns a row axis: the distinct union of change times,
admitted at exactly eight bytes per final timestamp and constructed in bounded
animation ticks. Closing or replacing the panel drops an unfinished builder.

`RowViewport` keeps the first row as `u64` and converts only the viewport-local
remainder to pixels. Layout exposes vertical and horizontal tracks/thumbs and
only intersecting columns. Preparation is capped at 256 rows (visible rows plus
overscan), 256 bytes per value preview and eight attribute previews. Details
and complete TSV copy are separate selected-row operations; details are
superseded by identity and copy is refused above 64 KiB. The accessible GPUI
projection contains only the recycled visible `ListBox`/`Option` rows.

The session memory budget is common to remote and native owners. Native input
images, decoded histories and generator indexes retain reservations in the
same ledger used by the table's 4 MiB panel reservation and optional signal
axis. This makes two tables, Waves and Pipeline share raw data without charging
or copying it twice.

Animations use an explicit clock: `tick(now)` advances every `Tween` (the shared
viewport, local viewports, pipeline row axes) and reports whether another frame
is needed, so tests are deterministic and a frontend only requests frames while
something moves.

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
monotonic panel IDs, panel content, and focus. Every trace opens into a
`Start` panel that names what the trace holds; the first content opened while
it is focused (sidebar rows, a stream or generator, a new tab or split) takes
its place under a fresh ID, so a panel never changes kind under a frontend's
view. Rows added while a pipeline or settings panel is focused go to the first
waveform panel in layout order, or open one. Split clones rows and formats
while retaining shared immutable histories; a new tab starts empty. Close
collapses empty groups and one-child splits, preserving surviving size ratios.
Every panel closes; the last content panel gives way to a start panel, and
`⌘W` on a lone start panel closes the trace. Layout proposals are
validated atomically for membership, duplicates, active tabs, finite positive
shares, depth and panel count. `debug_state()` reports one line per panel in
layout order, with its ID, focus and link flags before the waveform state.

`Document` owns the session, shared viewport and cursor, markers with stable IDs
and optional labels, translators, and load generations. Every timed panel embeds
a `nav::NavState`: two link flags choosing between document navigation and local
navigation through effective accessors, the local viewport tween and the local
cursor, and the time-axis commands (zoom about a pixel, fit, go to, pan) every
panel kind shares. Each `WaveModel` adds its rows, selection, columns, scroll and
transient input. Linked viewports share one animation; `App::tick` advances it
once, plus every independent animation. Unlink snapshots the displayed shared
value; relink adopts the retained shared position even if no panels currently
follow it. Markers always use the focused panel's effective cursor.

Pointer commands name a panel; keyboard actions and sidebar additions resolve
focus when handled. Deliveries fan out shared history Arcs to all matching
panels. Adding a loaded signal in another panel reuses the history without
queuing another decode. New panel IDs advance the existing allocator. Restoring saved IDs bumps the
document generation; frontend callbacks carry that generation, so delayed
pointer and menu events cannot target a replacement panel.

`Hierarchy::find_scope`, `find_generator` and `find_var` resolve literal path segments and return
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

## Hierarchy browser

The upper sidebar is a virtualized container tree: RTL/ESL scopes and transaction
streams in declaration order. `ScopeRole` distinguishes streams without duplicating
their kind string. The lower `MemberListModel` lists scope variables or stream
generators, including log sites. `TrackRef` is the same identity used by the raw
transaction catalog. Enum references and scope component names survive VTR and
FST loading; enum tables are not sidebar rows.

The core owns filtering, multi-selection, keyboard navigation, tooltips, icon
mapping and activation. Whole-trace search returns variables, generators, then
streams, with a combined 5,000-result cap and an explicit truncation indicator.
Changing containers exits whole-trace search. Enter activates selected members;
with no selection it adds only listed variables. The plus button always adds
only variables. Generator and stream activation opens a pipeline panel per
distinct track (see below); log sites report a notice, log panels remain future
work.

GPUI paints uniform rows, stream tags, port glyphs, severity badges and the
selected container breadcrumb. Lucide assets are bundled. Three sidebar tint
tokens serve streams and directions, with host palette contrast correction for
normal, selected and hovered rows. egui retains its minimal waveform feature set,
rendering shared members and glyphs without transaction activation UI.
Tree flattening uses an explicit stack. Saved selected/expanded paths cover
streams and retain unresolved names. Search-everywhere is transient.

Raw metadata includes container roles, component names, enum references,
generator declarations/attributes and the producer's time unit. Remote framing
version **3** rejects older peers before decoding the changed metadata schema.
Icon, tint, badge, selection and activation decisions stay on the client.

## Pipeline panel

`PanelKind::Pipeline(PipelineModel)` draws the transactions of one stream (all
its generators in catalog order) or one generator as Konata-style rows of stage
cells: one row per transaction in the resident (begin, end, id) order, one cell
per stage on the primary lane (`0` when present, else the most used lane), an
overlay band over the lower part of the row for every other lane, one grey cell
over the lifetime of a transaction without stages. `TxStatus::Aborted` rows are
tinted and marked in the label column; `Open` rows end with a dashed edge. The
label column shows the row index and the joined `vtr.label` text. Any stream
or generator opens; the `PIPELINE` kind is only a recognition hint for the
sidebar icon, the View ▸ Pipeline submenu and the command palette. The design
is [docs/pipeline-view.html](../../docs/pipeline-view.html).

The X axis is the document's time: the panel embeds a `NavState`, so the link
flags, the shared viewport and cursor, markers and the `waves.animation` setting
apply exactly as they do to wave panels, and a click sets the cursor to the
integer cycle under the pointer. The Y axis is local: `pipeline::RowView`
(fractional top row, row height at interface zoom 1.0) with the same
zoom-about, pan and 20 % edge-space clamp as `Viewport`, animated through the
same `Tween`. A mouse wheel zooms both axes about the pointer by one factor
(`2^(dy/100 px)`, animated to the accumulated target); a trackpad's precise
deltas pan both axes, and zoom with Ctrl/⌘ held (browsers report every wheel
as precise pixels); Shift+wheel pans time; pinch zooms both axes immediately;
a left drag past three pixels pans both axes, a shorter press is a click.
Keyboard actions are the wave panel's: `= -` zoom both axes, `F C Home End`
and the arrows move time, `↑ ↓` scroll three rows, `M ⇧M` markers, `L ⇧L`
links, Escape cancels a drag then the cursor. Row selection, formats and edge
actions are no-ops. No new settings or actions exist.

`Command::OpenPipeline { track }` (from `ActivateMembers`, the menu, the
palette) focuses the panel already showing the track or opens one split below
the focused panel. `PipelineModel::attach` retains the track through
`Document::retain_track`; a split copies the view and retains it again; closing
a panel (`Panels::close` returns the removed panels) detaches it, so the last
consumer releases the load and a late delivery is a no-op. The model never
copies records: `Rows` is a view over the loaded generators' slices with prefix
sums, `Loading`, `Failed` (with a retry button), `Unresolved` (a saved path
that is not a track of this trace) or `Unavailable`. Remote sessions need no
protocol change: `LoadRequest::Track` already delivers complete objects.

`PipelineLayout` is computed per frame from the bounds, the zoomed row view
(clamped and written back), the row count and the markers; it yields the
label, cells and divider rectangles, the marker chips, the visible row range
and the density step. Below two pixels per row the painter paints every
`step`-th row at the step's combined height, so the number of painted rows
never exceeds half the panel height in pixels; a step shows the flush of any
of its rows. Painting is `O(painted rows × stages per row)`: transactions and
stages outside the time window are skipped, cells narrower than a pixel are
widened to one, stage names appear from ten pixels per row in cells wide
enough for the shaped name, labels from seven. `StagePalette` assigns each
primary-lane stage name a hue from a ladder by first appearance (a VDB stage
table later fills the same struct). The header ticks, cursor line and chip,
and marker lines and chips come from `wave::overlay`, shared with the wave
painter. Times are formatted through `TimeBase`: the timescale exponent, or
the producer's `time.unit` file attribute (`cycle`) when it names one.

Workspace files save a `"pipeline"` panel: the track path, links, local
viewport and cursor, `rows` and `label_width`; restore resolves the path
against the track catalog and keeps unresolved tracks as an empty state that
is written back unchanged. The autosave stamp covers the same fields. GPUI
hosts the panel in the same `PanelCanvas` element and dock view as waves;
egui builds against the kind and paints whatever the core produces.

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

Machine state (the recent traces and workspaces) is `state.json` in the config
directory, versioned and written only by the app. Tests and the egui frontend
do not opt into workspace storage.

## User settings

`settings::registry` declares every setting once: id (`waves.snapPixels`),
page, group, title, description, keywords, kind and constraints, default,
apply mode and the hosts it exists on. `settings::Store` resolves three layers,
registry defaults, the user's `settings.json` and host overrides (VS Code's
`volna.*` configuration), into the typed `Settings` struct plus diagnostics.
The file is JSONC (comments, trailing commas, a byte-order mark) parsed by
`settings::jsonc`, which records the byte span of every top-level property so
a GUI change replaces one value span, an absent key is inserted before the
closing brace with the document's indentation and no trailing comma, and a
reset removes one property. Comments and unknown keys survive every edit. An
invalid value falls back to its default with a diagnostic naming the line; a
syntax error keeps the last good values and makes the editor read-only until
the JSON view fixes it. Renamed ids are rewritten once with a comment. The
file has no version field on purpose: each key degrades on its own, so a typo
never resets every setting (the AGENTS.md versioning rule applies to the machine-written
`state.json`, which is versioned and strict).

The store also owns the write queue: one write in flight, acknowledged by
ticket through `App::settings_saved`, coalesced after 300 ms, flushed on quit.
The SHA-256 of the last written bytes suppresses the file watcher's echo.
`settings::search` ranks entries for a query (`@modified`, `@page:`, `@id:`
filters; word, prefix, substring and fzf-like subsequence matches over title,
id segments, keywords and description) and reports the matched ranges.
`settings::schema` emits the JSON Schema written beside the file as
`settings.schema.json`, VS Code's `contributes.configuration` (a test keeps
`package.json` equal to it) and the read-only default document.

The Settings tab is a core panel kind (`PanelKind::Settings`) so focus, tab
order and dock placement follow the usual rules; `Panels::saved_view` leaves
it out of workspace files and it survives trace changes and restores. The
GPUI frontend renders it with gpui-kit's `Settings` pages, its own search bar
and flat ranked results, per-item modified bars and actions, and the JSON
view on gpui-kit's `Editor` with completion, hover and diagnostics from the
registry (Tree-sitter colours natively, the stub on the web). ⌘K opens a
command palette listing actions with their key hints and the ranked settings;
a boolean setting toggles in place, any other reveals the tab filtered to
`@id:`. Native watches the config directory (`notify`) for `settings.json`
and `themes/*.json` palette files, which `appearance.theme` selects by file
stem; the bundled palettes under `volna-core/assets/themes` (Dracula,
Catppuccin, GitHub, VS Code Modern) resolve through the same path. Standalone web keeps the document in `localStorage`. Inside VS Code the
extension owns the settings UI: ⌘, forwards to VS Code's editor filtered to
`@ext:vtr.volna`, and `onDidChangeConfiguration` pushes the `volna.*` keys to
`set_settings`, so a change applies without reopening the trace. The
`SettingsChanged { keys }` event lets frontends re-project the theme or the
workspace policy only when a relevant key changed.

### Interface zoom

`appearance.zoom` (0.5–3.0, step 0.1) is one multiplier over every size the
viewer already has; it replaces nothing. The core `Theme` carries its metrics
(`ui_size`, `row_height`, `timeline_height`, bar heights, `icon_size`,
`splitter_grab`, …) plus a `zoom` field; `Theme::zoomed(factor)` returns the
palette with every metric at its design size times the factor, whatever zoom
the input had, and `Theme::scale(px)` is a design pixel at that zoom. The wave
layout takes the zoom with the theme (`LayoutInput::zoom`, kept in
`WaveLayout::zoom`) and multiplies its constants (column minimum, splitter
tolerance, scrollbar, badge and chip sizes), so the painted rectangles and
the hit regions agree at every factor; the painter scales its paddings, chip
and label heights, the tick spacing and the trace inset the same way and
keeps hairlines at one pixel. Values saved in workspaces stay at zoom 1.0:
column widths and the sidebar width are multiplied when laid out and divided
when a drag stores them, `waves.snapPixels` is multiplied when a click snaps,
and a wave panel rescales its `scroll_y` when the row height changes so the
same rows stay on screen. The time axis of the waves (`Viewport`) is untouched.

The GPUI frontend keeps the installed palette at its design sizes and
re-projects `base.zoomed(zoom)` on every `SettingsChanged` naming the key,
exactly like a theme change (`theme::set_zoom`; nothing in the workspace is
rebuilt). The component theme's `font_size` becomes the zoomed UI size, and
gpui-kit's `Root` sets the window's rem size from it, so its settings editor,
inputs, menus, dialogs and the palette scale through rem units; Volna's own
chrome reads the zoomed theme metrics and writes literal sizes through
`ThemePx::px` (`t.px(12.0)`) instead of `px(12.0)`. `SettingsCommand::Zoom`
(`ZoomStep::{In, Out, Reset}`) steps the setting by 0.1, clamped and rounded
to the step, writing `settings.json` through the ordinary path (reset removes
the key), so ⌘= / ⌘+ / ⌘- / ⌘0, the View ▸ Appearance menu and the palette
persist across restarts. Embedded in VS Code the host's own window zoom
applies and the key bindings are off, but `volna.appearance.zoom` is still
honoured through the configuration path.

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

`Session::capabilities()` reports supported data independently of recorded
content. VTR supports transactions and relations even for an empty recording;
FST and the synthetic waveform source support waveforms only. `tracks()` is
resident metadata. `load_track()` is the single backend operation for a complete
stream or generator, including records, typed attributes, events,
stages, parent locations and incident relations.

Both local and remote consumers query the resulting immutable `LoadedGenerator`
objects. Their indexes provide transaction lookup and inclusive overlap queries,
including long intervals and point events. Relation identities preserve parallel
edges and references into unloaded tracks. No reader query facet is exposed to
viewer consumers: once an object is loaded, navigation reads resident storage.
Names are resolved at the backend boundary, so loaded data outlives the session.
The main frontend still has no transaction panel.

### Complete-object remote loading

The [client-server design](../../docs/client-server-simple.html) uses the same
resident objects for local and remote viewing. Native `OpenSpec::Path` loads
in process. In VS Code, the workspace extension launches `volna-server` beside
the file and relays opaque framed binary messages to `OpenSpec::Remote`.
Opening transfers full raw metadata; selecting signals or transaction tracks
loads their complete recordings. Pan, zoom, cursor and resident-data queries
have no network dependency. VDB and presentation stay on the client.

The server opens recordings through the same `OpenSpec::open` and calls the
same `Session::load_signals` and `load_track` methods as local execution. Its
additional responsibilities are wire conversion, framing and snapshot checks.
Track serialization borrows immutable records and relations, without a second
owned copy of their typed attributes.

Both executors consume `LoadRequest` and return `LoadResult`. The remote client
accepts requests through `submit`; submission failures return ordinary results
with the original identities. The browser bridge lives as long as the open
recording: a new open or a close drops it, while a workspace restore, which
also advances the document generation to invalidate earlier history results,
keeps the connection because the remote session is unchanged, so restored rows
and pipeline panels load over the same child. The document owns queued-demand filtering for
both paths. The browser bridge handles host calls and scheduling, not per-kind
load policy. `remote::client::RemoteClient` queues one command at a time. Responses carry
session/request identities and use checksummed LZ4 frames with fixed bincode
fields. Each frame is acknowledged after consumption. Cooperative decoders
build objects privately, validate references and construct transaction indexes;
only the explicit End produces a normal `LoadResult`. The browser groups bounded
decoder steps into short (2 ms target) work slices and yields via MessageChannel
tasks, avoiding nested-timer throttling while giving input and painting regular
opportunities to run. Track results also retain
the document's request identity, so removal/re-add and retry reject stale work.
Completed objects remain usable after disconnect; unfinished loads fail.
Both the document queue and the remote executor discard queued work after its
last consumer is removed. Active responses finish normally and are discarded
if no consumer remains; removing demand needs no cancellation protocol.

Signals and generators own reservations from a shared client memory budget.
Shared consumers keep the same storage alive; the last consumer releases it.
Admission and per-object limits are configurable in VS Code and apply on reopen.
Oversized objects fail explicitly, without truncation or automatic eviction.
These limits cover admitted data and conservative construction allowances,
not total browser RSS or backend reader caches. Full metadata and all selected
histories/tracks must fit; selecting a large stream can still be expensive.

## Frontends

The GPUI crate keeps the native app, the wasm page and the VS Code extension.
`Workspace` owns one `App` and is the only GPUI entity that holds viewer state.
It renders the chrome with GPUI elements and GPUI Kit's `gpui-component`
controls, registers GPUI actions that dispatch core `Action`s, keeps the popup
menu and filter input in step with the core's menu and filter, and spawns each
`LoadRequest` on the background executor. The window hosts `Workspace` inside
the component `Root` for overlays and focus routing. Buttons, tooltips and menus
use library interactions; `src/ui` adds Volna styling/placement and retains the
custom filter input, splitter, icons and headers. The window chrome follows
Zed: the title bar is Volna's own on every desktop, drawn over a transparent
system title bar. On macOS the native traffic lights sit in it; on Linux the
main window asks for client-side decorations (`VOLNA_WINDOW_DECORATIONS=server`
opts back in to the window manager's frame) and `src/ui/window_controls.rs`
draws the minimize, maximize/restore and close buttons on the sides GNOME's
`button-layout` configures, limited to what the compositor supports, with the
compositor's window menu on right-click; on Windows the same buttons mark the
platform's caption hit regions. The component `Root` supplies the Linux
client-side frame, shadow and resize edges. The filter stays custom on
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
| view | a `PanelKind` with a model in `volna-core`, a layout in `App::layout_panel` and a painter into `Scene`; the shared `PanelCanvas` paints it (the pipeline panel is the template) |
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
  cross-origin isolation or extra startup flags. VS Code relays complete-object
  frames over `postMessage`; browser file-picker inputs still use
  `OpenSpec::Bytes`. `debug_state()` on the wasm
  module logs the core state to the console for browser-driven verification.
- Native: `OpenSpec::Path` memory-maps the file; signal histories are loaded on
  demand off the UI thread when a variable is added.
- egui: the workspace builds `volna-egui` with `opt-level = 2` even in dev, so
  its software-rasterized tests run in seconds.
