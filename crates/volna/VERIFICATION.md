# Verification record

## Core/frontend split and egui frontend (2026-09-12)

The viewer was restructured into `volna-core` (no GUI toolkit), the GPUI
frontend in this crate, and a new native egui frontend (`volna-egui`). Every
check below ran on the same Apple Silicon Mac (macOS 25.6, Rust 1.96 stable,
`gpui-pre` 0.3.4, egui/eframe 0.36.2, Chrome 153, VS Code 1.137).

Automated:

- `crates/volna/check.sh` passes: formatting and strict Clippy for the three
  viewer crates; 42 `volna-core` tests (27 unit tests including the theme
  suite, 15 headless viewer tests in `tests/headless.rs`); 3 GPUI adapter tests
  plus the Metal viewer integration test with its unchanged interaction
  assertions; the egui headless screenshot test; the three JavaScript
  extension tests.
- Headless core tests cover: alias rows sharing one pending/loaded history,
  stale results by generation, retry after failure, latest open wins, close
  invalidating late success and error, open errors, cursor/marker/selection
  through the document, deterministic zoom/pan/fit with an explicit clock,
  wheel zoom and drag pan, dense-column collapse (one band for a 1000-change
  burst, twenty edges when zoomed in), the format menu and translator cycle,
  sidebar keys and filtering, layout hit regions and scene cursors, hover
  repaint coalescing, and the `debug_state` format.
- `cargo check -p volna-core --target wasm32-unknown-unknown` and
  `cargo clippy -p volna --target wasm32-unknown-unknown --lib --all-features
  -- -D warnings` pass with Homebrew LLVM 20 for zstd-sys.
- `web/build.sh` rebuilds the wasm bundle and the extension media (fonts and
  icon licences now come from `volna-core/assets`).

Visual, GPUI native (`VOLNA_SCREENSHOTS=results crates/volna/check.sh`): the
fourteen captures of the previous record were regenerated with identical state
strings at every step. The empty-state "list-tree" icon now paints; before the
split its SVG was missing from the asset table and failed silently.

Visual, standalone web and VS Code: headless Chrome and the Extension
Development Host were driven with `tools/cdp.mjs` on the rebuilt bundle. The
new `debug_state()` export confirms the core state from the browser console.
Web: load, scope select, `+`, cursor click, row select, `=` twice, marker,
ctrl+wheel zoom, badge menu (`menu=true`, `Escape` → `menu=false`), `End`
(`results/web/01-loaded.png` … `06-end.png`). VS Code: the custom editor opened
`picorv32.vtr` under the host theme; scope select, `+`, cursor, row select,
zoom, marker, next edge, badge menu and fit (`results/vscode/02-viewer.png` …
`06-fit.png`).

Visual, egui (`cargo test -p volna-egui --test screenshots`, software
rasterized at 1440×900): empty state, loaded trace, 29 signals added with `+`,
cursor and marker after zoom, the format menu, and the variable filter
(`crates/volna-egui/results/egui/01-empty.png` … `06-filter.png`). The test
also asserts the sidebar sash drag, the names-divider drag, pinch zoom, fit,
right-drag pan and typing into the variable list.

Frame time before and after the split, same machine, same harness (see
"Performance" below for the method): the after column is the current build.

| transitions (busiest signal) | before: fit median / p90 | after: fit median / p90 | before: zoomed 64× | after: zoomed 64× | table paint before / after |
|---|---|---|---|---|---|
| 10 K | 1.67 / 1.78 ms | 0.85 / 0.91 ms | 1.73 / 1.76 ms | 0.87 / 0.88 ms | 0.63 / 0.63 ms |
| 1 M | 1.10 / 1.13 ms | 0.55 / 0.56 ms | 1.10 / 1.12 ms | 0.54 / 0.56 ms | 0.36 / 0.36 ms |
| 100 M | 1.76 / 1.82 ms | 0.87 / 0.89 ms | 1.76 / 1.79 ms | 0.87 / 0.89 ms | 0.69 / 0.69 ms |

The wave painter's own time is unchanged (same algorithm, now emitting a
display list). Whole-frame time roughly halved because the GPUI adapter caches
shaped text per string and colour across frames, where the previous element
reshaped every label on every paint.

Limits: the egui captures are software rasterized (no GPU feathering
differences are checked), the egui frontend was not exercised in a real window
by this record, Linux/Windows were not run, and browser checks remain spot
checks rather than pixel baselines.

## Automated checks (2026-09-12)

- `crates/volna/check.sh`: formatting, strict Clippy across all targets/features,
  all 21 unit/regression tests, and the macOS viewer integration test pass.
- `tests/viewer.rs`: interaction assertions and nonblank Metal captures pass;
  optional PNG output was checked, including the format menu. `--list` and the
  separate ignored `frame_times` test also pass. Captures are not compared to
  pixel baselines; timing output is diagnostic, not a performance assertion.
- The WebAssembly library also passes `cargo check --all-features`; native
  test dependencies are not enabled on that target.
- `cargo run --locked -p volna --all-features -- --help`: the native executable
  starts and reports `usage: volna [FILE.vtr] [--synthetic N]`.
- `sh crates/volna/web/build.sh`: optimized wasm build and wasm-bindgen generation
  pass; the VS Code media bundle includes the wasm module and asset licenses.
- The five core workspace packages pass `cargo check`.
- Shell and JavaScript syntax checks pass. Generated wasm/VS Code bundles remain
  ignored. Live graphical acceptance is separate from these automated checks.

## Loading ownership regression checks (2026-09-12)

`cargo test -p volna --offline --locked` passes 21 tests, including five GPUI state
tests. Delayed completions verify that a newer open or close wins over an old
success/error and that a stale demo cannot add rows. Signal tests cover stale
results, one load for duplicate/alias rows, shared history ownership, release on
source replacement, and retry after failure. These run without a GPU or screenshots.
`cargo clippy -p volna --offline --locked --all-targets --all-features -- -D warnings` passes.
`cargo check -p volna --offline --locked --target wasm32-unknown-unknown --lib` also passes
with `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown` pointing to
Homebrew LLVM 20's `clang` and `llvm-ar`.

## Visual verification

Screenshots under `results/` are local generated artifacts, ignored by Git;
the paths below identify capture outputs, not files included in the repository.

Recorded on 2026-09-12 on an Apple Silicon Mac (macOS 25.6, Rust 1.96 stable,
`gpui-pre` 0.3.4, Chrome 153, VS Code 1.137).

Screen recording is not granted to the terminal on this machine, so no
screenshots were taken of live windows. Every image below was produced by the
application itself or by the hosting browser through its debugging protocol:

- native: GPUI's Metal headless renderer (`VOLNA_SCREENSHOTS=results cargo test -p volna --features visual-test --test viewer`),
  driven with real `MouseDown`/`MouseUp`/`KeyDown` platform events;
- web: headless Chrome with `--remote-debugging-port`, driven with
  `Input.dispatchMouseEvent` / `Input.dispatchKeyEvent` and captured with
  `Page.captureScreenshot` (`tools/cdp.mjs`);
- VS Code: the Extension Development Host started with `--remote-debugging-port`
  and driven the same way, so input goes through VS Code into the webview.

## Feature checklist

Legend: N = native, W = web (Chrome), V = VS Code webview. All three were exercised
with the bundled `examples/picorv32.vtr` (427 signals, 42 519 changes).

| Feature | N | W | V | Evidence |
|---|---|---|---|---|
| Scope tree: expand/collapse, select, keyboard | ✓ | ✓ | ✓ | `results/02-loaded.png`, `results/web/01-loaded.png`, `results/vscode/02-viewer.png` |
| Variable list for the selected scope, filter box, add all (`+`), double-click add | ✓ | ✓ | ✓ | `results/03-signals.png`, `results/web/02-signals.png`, `results/vscode/03-signals.png` |
| Names / values / waves columns, pixel-aligned rows | ✓ | ✓ | ✓ | same |
| 1-bit signals (edges, high fill, dense regions) | ✓ | ✓ | ✓ | `results/04-cursor-zoom.png`, `results/web/04-wheel.png`, `results/vscode/04-cursor-zoom.png` |
| Buses (hexagon segments, value text, `x` colouring) | ✓ | ✓ | ✓ | same |
| Translators: bit, hex, binary, unsigned, signed, float (menu and `T` cycle) | ✓ | ✓ | ✓ | `results/05-format-menu.png`, `results/web/05-menu.png`, `results/vscode/05-menu.png`; `bin` badge in `results/web/04-wheel.png` |
| Cursor: click/drag with edge snapping, value column follows cursor, time chip | ✓ | ✓ | ✓ | `results/04-cursor-zoom.png` and web/vscode equivalents |
| Markers (`M`, chip, jump, `shift`-click remove) | ✓ | ✓ | ✓ | `M1` chip in the same images |
| Zoom: `=`/`-`, `⌘`/`ctrl` + wheel about the pointer, animated | ✓ | ✓ | ✓ | same |
| Pan: wheel, drag, `←`/`→`; fit `F`; edges `Home`/`End`; centre on cursor `C`; next/prev edge | ✓ | ✓ | ✓ | `results/web/06-end.png`, `results/vscode/06-fit.png` |
| Splitters (sidebar, scopes/variables, names/values) with hover highlight and resize cursor | ✓ | ✓ | ✓ | exercised natively; same code path on web |
| Empty state and loading state | ✓ | ✓ | ✓ | `results/01-empty.png`; spinner shown while loading |
| File open: native dialog / drag-drop; web `<input type=file>`; VS Code `showOpenDialog` and custom editor | ✓ | ✓ | ✓ | VS Code custom editor opened `picorv32.vtr` (`results/vscode/02-viewer.png`) |
| Crisp text at 1× and 2× | ✓ | ✓ | ✓ | native and VS Code captures are 2×, Chrome capture is 1× |

## Performance

Frame time measured with the headless renderer at 1440×900 logical pixels
(2880×1800 device pixels), ten signals of the synthetic trace visible, one
keyboard pan per frame so every frame repaints. `table_paint_ms` is the wave
table's own paint time; the frame figure includes layout, the sidebar and GPU
submission.

| transitions (busiest signal) | fit-to-view median / p90 | zoomed 64× median / p90 | table paint |
|---|---|---|---|
| 10 K | 0.85 / 0.91 ms | 0.87 / 0.88 ms | 0.63 ms |
| 1 M | 0.55 / 0.56 ms | 0.54 / 0.56 ms | 0.36 ms |
| 100 M | 0.87 / 0.89 ms | 0.87 / 0.89 ms | 0.69 ms |

Frame time is flat with respect to trace length. The 100 M row is slightly
higher only because the procedural history hashes each probe; every viewport
costs O(columns × log n) searches. A 4K viewport (≈ 1.3× the columns) stays far
below the 16.7 ms budget. Measured after the core/frontend split; the previous
numbers are in the section at the top of this file.

Reproduce with:

```sh
cargo test -p volna --profile viewer --features visual-test --test viewer frame_times -- --ignored
```

## Acceptance check (look and feel)

At the pre-host-theming baseline, all colours were Zed One Dark tokens (`src/theme.rs`); fonts are IBM Plex Sans
and Lilex as shipped by Zed; icons are Lucide at 16 px; every panel header is
32 px with a 1 px `border_variant` rule; rows are 24 px on a 4 px grid.
`results/vscode/04-cursor-zoom.png` shows the viewer inside VS Code's chrome at
2×: type sizes, row heights, header weights and border widths sit next to VS
Code's own without visible mismatch. A live side-by-side with Zed was not
captured because screen recording is not available to this session; the
theme tokens, fonts and metrics are Zed's own, so that comparison is a manual
step for the reviewer.

## Known limitations

- Text input is a minimal filter field (no IME, no selection).
- Web target is single-threaded (loading runs on the main thread; fine for
  the bundled examples, noticeable for very large files).
- No `wasm-opt` pass; the bundle is 12.8 MB, mostly fonts and the VTR reader.
- Marker chips can overlap tick labels at some zoom levels.

## Unified host theming (2026-09-12)

The final implementation combines precomputed surfaces with one shared Rust CSS
parser and VS Code mapping. All four raw VS Code 1.137 snapshots and the mixed
JSON palette run through the production adapters, not a generated intermediary.

Validation:

- `crates/volna/check.sh`: formatting, strict native Clippy, 32 Rust unit tests,
  the Metal interaction test and three JavaScript transport/lifecycle tests.
  The timing test is deliberately ignored. On macOS the visual test needs access
  to GUI services; it cannot run inside a sandbox that denies those services.
- Strict WASM Clippy: `cargo clippy --locked -p volna --target
  wasm32-unknown-unknown --lib --all-features -- -D warnings`, with Homebrew LLVM
  20 configured for zstd-sys. `web/build.sh` rebuilds the optimized WASM and
  extension media from this branch.
- The Metal harness verifies unchanged source/interaction diagnostics through
  sparse, mixed, and all four real palettes with an open format menu. Captures
  include the mixed-theme empty-table state, which now has a single background
  under its hint. Mixed surfaces, menu text, HC outlines and value colours were
  visually inspected.
- A fresh isolated VS Code profile loaded the rebuilt extension and bundled
  `examples/picorv32.vtr`. Live switches covered Dark Modern, Light Modern,
  Default High Contrast, Default High Contrast Light, a light editor with dark
  sidebar and blue status bar, and removal of those customizations. All six
  switches kept the identical document and canvas. Captures retain eight signals,
  selection, cursor at 3.371 µs, zoom and a marker.
- Standalone Chrome loaded the same trace automatically with One Dark. Actual
  browser checks also exercised `volnaHostTheme()` with the mixed JSON fixture,
  a live `set_theme(json)` update, rejection of malformed JSON, and startup with
  a VS Code snapshot missing its kind but containing a light editor background.
  Each produced a canvas without an explicit `start()` call or metadata timer.

Local captures are in `/tmp/volna-unified-shots/` (not committed). Repeat the
native captures with `VOLNA_SCREENSHOTS=<directory> crates/volna/check.sh`.
For live checks, build the web bundle, start VS Code with a temporary
`--user-data-dir` and `--extensionDevelopmentPath=<absolute vscode-ext path>`,
open `examples/picorv32.vtr`, add signals/cursor/marker/zoom, then switch themes
and `workbench.colorCustomizations` in that temporary profile. Reopen with Volna
if the file initially uses another editor. Serve `crates/volna` locally and open
`web/?file=../examples/picorv32.vtr` for the standalone check.

Limits: browser and visual checks are spot checks, not pixel-baseline tests or a
frame-by-frame proof of no startup flash. Linux/Windows, other GPU backends and a
matrix of third-party themes were not exercised. Host text choices are preserved;
contrast safeguards are not a complete accessibility audit. Fonts/metrics remain
Volna's bundled defaults. No Zed integration or native OS-theme following was
added. The existing `block` dependency still emits its future-Rust warning.

## FST input validation

On 2026-09-12, `crates/volna/check.sh` passed with macOS desktop access:
formatting, all-target/all-feature Clippy, 54 core tests, three GPUI unit tests,
the Metal interaction test, two egui tests and three Node adapter tests.
`crates/volna/web/build.sh` also passed with the FST dependency and regenerated
the wasm/VS Code bundles. The optional frame-time benchmark remains ignored;
this verifies compilation and native runtime behavior, not a browser runtime
matrix. The pre-existing `block` dependency still reports a future-Rust warning.

The headless FST tests include reference-writer fixtures with binary strings,
reals, nine-state logic and aliases, both plain and gzip wrapped. Wrapped and
plain histories agree sample-for-sample. Section framing is checked before
fst-reader takes ownership: truncated section headers/payloads, unfinished
lengths and arithmetic overflow return errors. This is framing validation,
not a claim of exhaustive validation of malicious compressed payloads.

The `fst_types.fst` regression loads all 76 variables, including raw EVCD port
payloads and event occurrences; the egui loading smoke test uses this file.
The reference-writer event test checks repeated occurrences and marker-only
painting, including an empty viewport between occurrences.

Run `cargo test -p volna-core --test fst`. Fixture regeneration commands live
with `crates/volna-core/tests/fixtures/fst_values.c`; regular tests need only
the committed fixtures. The adapter decompresses gzip wrappers into memory
before applying the same metadata checks to their inner recording; normal
native streaming-file memory expectations do not apply to wrappers.

The FST tests also compare values at every change timestamp in the committed
Verilator `features`, `operators` and `pipeline` FST/VTR pairs. Tests cover
path/byte opening, duplicate/alias history sharing, unknown signals, empty
batches, unavailable initial samples and explicit event/dump-off/time-offset
errors. Transaction contract tests distinguish unsupported FST operations from
empty VTR results, and check VTR attributes, events, stages, parents, inclusive
overlap boundaries, filtering, early stopping and cross-stream relations.
The egui test opens an FST path and the GPUI test opens FST bytes; both load
every variable on their production executors and render nonblank frames. These tests extend,
rather than replace, the existing VTR interaction tests.

### FST loading and VTR regression measurements (2026-09-12)

Apple M5, Rust 1.96, macOS; temporary Cargo harnesses outside the repository,
release optimization, thin LTO, one codegen unit. Baseline was clean HEAD
`a1aaf9f`; both harnesses resolved the same shared dependency versions offline.
Each repetition opened a fresh session, selected 32 evenly spaced unique
signal identities in sorted order, loaded full histories and dropped the
session/results. Current used `load_signals`; baseline used its existing
per-signal loop. Five repetitions per process, no cache flushing or CPU
affinity; best opening and loading times are reported separately. These are
warm local measurements, not cold-storage or frame-time guarantees.

| Input | Bytes | Selected changes | Best open ms | Best load ms | Peak process RSS bytes |
|---|---:|---:|---:|---:|---:|
| SCR1 FST | 3,648,879 | 916,224 | 0.289 | 25.360 | 75,939,840 |
| RSA long FST | 338,245,966 | 6,043,117 | 0.337 | 1,754.595 | 1,584,316,416 |
| SCR1 VTR, HEAD | 1,244,845 | 916,224 | 0.292 | 4.499 | not measured |
| SCR1 VTR, current | 1,244,845 | 916,224 | 0.336 | 4.397 | not measured |
| RSA long VTR, HEAD | 325,338,845 | 6,043,117 | 0.089 | 111.458 | not measured |
| RSA long VTR, current | 325,338,845 | 6,043,117 | 0.101 | 110.150 | not measured |

Inputs: `ext/wavepeek/web/playground/assets/scr1_axi.fst`,
`bench/workloads/gen/rsa256_long.fst`, and
`bench/out/shared-loads/{scr1_axi,rsa256_long}_shared.vtr`. Generated benchmark
files are local artifacts from the existing benchmark suite. Peak RSS comes
from macOS `/usr/bin/time -l` and includes the reader, allocator and all five
iterations, not just live history buffers.

FST load times over the five repetitions were 37.267, 27.158, 26.049, 25.360,
25.882 ms for SCR1 and 1826.575, 1786.478, 1782.329, 1789.205, 1754.595 ms
for RSA. Whole-history FST storage uses owned values and can consume much more
memory than the file: about 1.48 GiB RSS for this RSA selection. No bounded-memory
or remote-viewing claim follows from batched loading. VTR loading showed no
regression in these selections; opening pays a small additional track-metadata
cost. The lower RSA VTR timing is not claimed as an improvement. No encoding,
writer or VTR decoder implementation changed, so these measurements make no
file-size/write-speed claim and do not replace the format benchmark suite.
