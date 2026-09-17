# Volna — the VTR/VDB viewer

Volna is the official viewer for VTR and its separate VDB design/presentation
companion. The viewer itself is `volna-core`, a crate with no GUI toolkit; this
crate is its [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
frontend (GPUI Kit 0.6.1, with the `gpui-pre` 0.3.4 snapshot in the lockfile), which runs as a
native macOS app and, compiled to WebAssembly, inside a VS Code webview. A
second frontend, `volna-egui`, runs natively on eframe. See
[ARCHITECTURE.md](ARCHITECTURE.md) for the split.

The current implementation displays **VTR and FST waveforms**. VDB attachment, source
browsing, and transaction/pipeline views are planned. Keep static VDB semantics
separate from runtime VTR data as these features are developed.

FST opens directly through `fst-reader` in the shared core, including native
paths, browser byte inputs and the VS Code workspace server. Selection queues a batch of complete
signal histories; aliases share the loaded data. Supported values include
nine-state logic, reals and arbitrary byte strings. Byte strings display quoted
escapes. Enum signals display recorded numeric bits; enum tables are not used
as value translators. Event signals display point markers at recorded timestamps,
not held levels. EVCD port payloads display as escaped raw bytes, preserving
strength fields without treating them as logic bits. Files containing dump-activity records or a
nonzero time-zero offset are rejected explicitly: the current viewer cannot
faithfully display recording gaps or apply that offset. These checks also
apply inside gzip-wrapped FST files.

Event variables in VTR and FST render as upward arrows at each recorded
occurrence, following Surfer. Occurrences in the same pixel share one arrow
in a distinct coalesced-event colour (amber in the default theme). Zooming in
separates nearby occurrences; duplicates at the same timestamp keep that colour.
Events do not hold a level between timestamps. The VTR writer preserves event
occurrences automatically, independently of its value deduplication setting.

FST scope and variable names, types, direction, hierarchy and aliases are used.
Source stems, component names, comments, enum tables, VHDL type annotations,
array/pack attributes and other extended hierarchy metadata are not displayed
or exposed by the current waveform hierarchy. VDB/source browsing is separate
future work. Incomplete recordings requiring a `.hier` sidecar are unsupported.
Gzip-wrapped files are decompressed into memory; normal native FST files remain
buffered on disk. The current web build decodes on its single browser thread.

Panels: a hierarchy browser (scope tree plus a separate, filterable variable
list) and dockable waveform panels with three pixel-aligned columns (names,
values, waves), timelines, linked or independent navigation and shared numbered
markers. Split a panel to clone its rows, or create an empty tab. Drag tabs to
rearrange groups; the last remaining panel hides its tab header.

The `gpui-kit` 0.6.1 umbrella supplies the runtime, assets and components.
Standard buttons, tooltips and popup menus use its component module.
Menus support arrow-key navigation, Enter to choose and Escape to dismiss.
The filter retains the existing custom widget on native and web. Viewer state
and waveform rendering remain in `volna-core`; see [Architecture](ARCHITECTURE.md)
for the component boundary and single-threaded web configuration.

## Development checks

Use rustup with the dated nightly in the root `rust-toolchain.toml`. The WASM
dependency graph includes `wasm_thread`, which requires a nightly compiler even
though Volna selects a single-threaded runtime. Run `./check.sh` from
this directory (or `./volna/volna/check.sh` from the repository root) to check
formatting, deny Clippy warnings across all targets/features, and run the tests
of `volna-core`, `volna` and `volna-egui`. The check script also requires
Node.js 22+ for the VS Code adapter tests. The native build requires the
platform SDK, including Metal tools on macOS. On macOS, `check.sh` also runs the
feature-gated Metal integration test; `cargo test -p volna` runs only the
GPU-free tests. Run `cargo fmt -p volna-core -p volna -p volna-egui` to apply
the committed formatting policy. The viewer crates use the root workspace
lockfile. Root commands without `-p` build the core crates; use `-p volna`,
`-p volna-core` or `-p volna-egui` for the viewer, or `--workspace` to include
everything.

`cargo test -p volna-core` runs the headless viewer tests: they feed commands,
assert on model state and on the display list, and run on every platform.

## Build and run (native, GPUI)

```sh
cd volna/volna
cargo run -p volna --profile viewer -- examples/picorv32.vtr
cargo run -p volna --profile viewer -- --synthetic 100000000
cargo test -p volna
```

## Build and run (native, egui)

```sh
cargo run -p volna-egui --profile viewer -- volna/volna/examples/picorv32.vtr
cargo run -p volna-egui --profile viewer -- --synthetic 100000000
cargo test -p volna-egui       # headless interaction test; PNGs under volna/volna-egui/results/egui/
```

The egui frontend has no wasm build and no VS Code integration. It uses the
OS title bar and egui widgets for the chrome; the wave panel is the same core
painter. Its existing waveform shortcuts use the Command key on macOS; it does not
implement the main viewer's docking or workspace commands.

## Build and run (WebAssembly)

Requires Node.js, the `wasm32-unknown-unknown` target, `wasm-bindgen-cli`
matching the locked `wasm-bindgen` version, and a clang
that can target wasm32 for the `zstd` C sources (Apple's clang cannot; the
script picks up Homebrew LLVM automatically, or set `CC_wasm32_unknown_unknown`).

```sh
./web/build.sh                                     # → web/dist and vscode-ext/media
python3 -m http.server 8080                        # from volna/volna/
open "http://localhost:8080/web/?file=../examples/picorv32.vtr"
```

The page uses WebGPU when available and falls back to WebGL2. Volna selects
`gpui_kit::platform::single_threaded_web()` and builds ordinary, unshared WASM
memory, without atomics/shared-memory linker flags or a threaded standard
library. It does not require `SharedArrayBuffer` or cross-origin isolation,
including in default VS Code without `--enable-coi`. The module
exports `debug_state()`, which logs the viewer state to the console; browser
verification scripts use it.

## VS Code extension

```sh
./web/build.sh
code --extensionDevelopmentPath="$PWD/vscode-ext" "$PWD/examples"
```

Opening any `*.vtr` or `*.fst` file uses the viewer as a custom editor; the command
"Volna: Open Waveform Viewer" picks a trace and opens its custom editor. Package with `npx @vscode/vsce package`
inside `vscode-ext/`.

The build also places a native `volna-server` in `vscode-ext/bin`. The extension
runs on the workspace host, including SSH/container workspaces, and starts that
child beside the recording. Set `volna.serverPath` to a server executable built
for the workspace host when the bundled binary targets a different platform.
Only complete metadata and selected histories/tracks cross the relay; the
extension does not read and send the whole trace file. Navigation over loaded
data is local to the viewer.

`volna.remote.memoryMiB` controls client admission (default 512 MiB), and
`volna.remote.objectMiB` limits each decoded wire object (default 256 MiB).
Reopen the trace after changing either setting. A load that cannot fit fails
explicitly; remove loaded data or raise the limit. For a failed signal, click
its format badge and choose **Retry loading**. This retries the complete signal
for all alias rows without adding rows; other loaded signals remain available.
A disconnected recording must be reopened before new loads can succeed.
Save the workspace before closing to restore the same selections on reopen;
the new session reloads complete objects using their durable paths.
These limits do not bound
total browser memory or guarantee that a large allocation will fit its address
space. Metadata is subject to admission too. See the
[complete-object design](../../docs/client-server-simple.html).

VS Code colours follow the current theme, including custom themes, colour
customizations, light/dark and both high-contrast modes. Changes repaint the
existing viewer without reopening the trace or resetting interaction/layout
state. The page background follows VS Code while loading; GPUI reads the initial
palette synchronously before creating its window. Missing metadata uses available
colours and inferred appearance without delaying startup; late metadata still
applies. Fonts and dimensions remain Volna's bundled ones.
Native and standalone web continue to use One Dark.

## Saved workspaces

Native and VS Code reopen the layout, signal lists, formats, links, cursor,
markers and sidebar from `<trace-name>.volna.json`. Save Workspace As creates a
separate workspace; Open Workspace validates it before replacing the current
view. A workspace referencing another trace must be opened with that trace.
Missing signals and unsupported panel types are preserved, with a notice.

Native options: `--workspace FILE`, `--no-workspace`, `--config-dir DIR`.
`VOLNA_WORKSPACE=off|FILE` and `VOLNA_CONFIG_DIR=DIR` provide environment defaults.
The config directory is `$XDG_CONFIG_HOME/volna` or `~/.config/volna` on Unix and
`%APPDATA%/volna` on Windows. Read-only trace directories use per-user fallback
storage. A restored fallback stays active until an explicit destination change.
Unreadable or incompatible workspace files pause autosave to preserve the file.

VS Code contributes the same keys under `volna.*` (see below). Its file access
uses the extension host, including remote files. Ordinary browser file-picker
sessions and egui have no automatic workspace storage. Tests disable
persistence unless they explicitly create a temporary store.

## Settings

`⌘,` opens the Settings tab beside the waves; `⌘K` opens the command palette.
User settings live in `<config dir>/settings.json`, a JSON file with comments
that holds only the keys you changed. The tab edits that file surgically, so
hand edits, comments and formatting survive; "Open settings.json" (`⌘⇧,`)
switches the tab to a JSON editor with completion, hover documentation and
diagnostics, and `settings.schema.json` beside the file gives external editors
the same. The search box ranks settings fuzzily (`folcur` finds "Link new
panels" through its keywords) and accepts `@modified`, `@page:waves` and
`@id:waves.snap`. A changed setting shows a bar and offers reset, copy id and
copy as JSON. An invalid value falls back to its default with a diagnostic; a
syntax error keeps the last good values and disables GUI edits until fixed.

| Key | Values | Applies |
|---|---|---|
| `appearance.theme` | `one-dark` or the stem of a palette JSON in `<config dir>/themes/` | live (native, web) |
| `panels.linkByDefault` | boolean | new panels |
| `waves.animation` | `on`, `reduced`, `off` | live |
| `waves.snapPixels` | 0–24 | live |
| `workspace.autosave` | `sidecar`, `vscode` (VS Code only), `off` | next trace open |
| `workspace.recentLimit` | 5–50 | live (native, web) |
| `remote.memoryMiB`, `remote.objectMiB`, `remote.serverPath` | see the VS Code contribution | next trace open (VS Code only) |

Recent traces and workspaces are machine state, kept in `state.json` and never
in settings. Inside VS Code the extension owns the settings UI: `⌘,` opens
VS Code's settings filtered to Volna, and configuration changes reach the
viewer immediately. A version 1 `preferences.json` is migrated once into
`settings.json` and `state.json` and renamed `preferences.json.migrated`.

Main GPUI viewer shortcuts (Command on macOS, Ctrl elsewhere):

| Action | Keys |
|---|---|
| Split right / down | `⌘\` / `⌘shift-\` |
| Empty tab / close panel | `⌘N` / `⌘W` |
| Next / previous panel | `ctrl-tab` / `ctrl-shift-tab` |
| Focus panel 1–9 | `⌘1` … `⌘9` |
| Toggle viewport / cursor link | `L` / `shift-L` |
| Zoom a dock group | `shift-esc` |
| Save / Save As workspace | `⌘S` / `⌘shift-S` |
| Settings / settings.json | `⌘,` / `⌘shift-,` |
| Command palette | `⌘K` |

The panel menu also provides rename and close-other-panel actions.

## Using the viewer

| Action | Mouse | Keys |
|---|---|---|
| Add variables | double-click a variable; `+` in the Variables header adds all listed | `⏎` adds the selected variables |
| Search all variables | type in the filter with no scope selected | |
| Set cursor | click or drag in the waves or the timeline (snaps to nearby edges) | `shift-←/→` previous/next edge of the selected signal |
| Zoom | `⌘`/`ctrl` + wheel, pinch | `=` / `-`, `F` or `shift-F` fit, `shift-Z` zoom and centre on cursor |
| Zoom to selected area | `⌘`/`ctrl` + left-drag across a time range | `esc` cancels the selection |
| Pan | wheel/trackpad over waves, middle/right-drag | `←` `→`, `PageUp` later / `PageDown` earlier, `Home`/`S` start, `End`/`E` end, `C` centre on cursor |
| Markers | click a chip to jump, `shift`-click to remove | `M` add at cursor, `shift-M` clear |
| Value format | click the badge in the values column | `T` cycles binary / hex / decimal / signed / float |
| Rows | click, `shift`/`⌘` multi-select, drag the column dividers | `↑` `↓`, `⌫` remove, `⌘A`, `esc` |
| Sidebar | drag the dividers | `⌘B` toggle |
| Files | drag a `.vtr` or `.fst` onto the window | `⌘O` |

The status bar shows the trace range, cursor time, pixel resolution and the
smoothed paint time of the wave table.

`⌘`/`ctrl` + left-drag selects a time range in either direction. Vertical
movement does not change the action; a shaded preview shows the selected range
before release. Shift-wheel scrolls signal rows even over the waveforms;
wheel over names or values also scrolls rows. Plain left-drag scrubs the cursor.
Keyboard and wheel pans animate, and repeated inputs accumulate at the target
position. Middle/right-drag and pointer-anchored wheel/pinch zoom respond immediately.

## Verification and performance

```sh
cargo test -p volna-core                                   # headless core tests, every platform
cargo test -p volna --features visual-test --test viewer   # macOS Metal harness
# Optional PNG artifacts (from volna/volna/):
VOLNA_SCREENSHOTS=results cargo test -p volna --features visual-test --test viewer
# Optional timing measurements, excluded from the regular test run:
cargo test -p volna --profile viewer --features visual-test --test viewer frame_times -- --ignored
cargo test -p volna-egui --test screenshots                # egui headless screenshots
```

The integration test in `tests/viewer.rs` renders the production workspace
offscreen and asserts splitter, signal-loading, selection, cursor, zoom, marker,
and menu behaviour after real input events. It checks captures are nonblank;
PNG saving is optional and there is no baseline image comparison. The harness
runs on the macOS main thread using `libtest-mimic` (normal test filters and
`--list` work); other platforms report a skip. The same interactions are
asserted headlessly in `volna-core/tests/headless.rs` on every platform.

The separate ignored `frame_times` test measures synthetic traces of 10 K,
1 M and 100 M transitions without pass/fail timing thresholds. See
[ARCHITECTURE.md](ARCHITECTURE.md) for how rendering cost is kept proportional
to the viewport width, and [VERIFICATION.md](VERIFICATION.md) for coverage and measurement instructions.

Bundled fonts and icons live in `volna-core/assets` and retain their
[third-party licenses](THIRD_PARTY.md).
