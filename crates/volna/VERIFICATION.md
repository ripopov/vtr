# Verification record

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
| 10 K | 1.78 / 1.98 ms | 1.88 / 1.90 ms | 0.70 ms |
| 1 M | 1.10 / 1.13 ms | 1.10 / 1.14 ms | 0.36 ms |
| 100 M | 3.33 / 9.94 ms | 3.86 / 6.55 ms | 1.23 ms |

Frame time is flat with respect to trace length. The 100 M row is slightly
higher only because the procedural history hashes each probe; every viewport
costs O(columns × log n) searches. A 4K viewport (≈ 1.3× the columns) stays far
below the 16.7 ms budget.

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

## VS Code theme following (2026-09-12)

Validation for the host-theming change:

- `crates/volna/check.sh` passes formatting, strict native Clippy across all
  targets/features, 24 Rust unit/regression tests, the Metal viewer interaction
  test, and three Node.js adapter tests. `frame_times` remains intentionally
  ignored; no performance claim or VTR benchmark update is made.
- `CARGO_TARGET_DIR=/Users/ripopov/work/vtr/target sh crates/volna/web/build.sh`
  builds the optimized wasm and extension media bundle. The target-directory
  override reused local dependencies; it is not required for a clean build.
- Rust tests cover opaque/alpha tokens, sparse and low-contrast palettes in all
  four modes, text/wave/marker contrast, and preservation of source/history
  identity, selection, cursor, zoom, markers, column widths and scroll offset.
- Adapter tests cover CSS hex and rgb()/rgba() values, missing/malformed values, all four theme
  classes (including HC light's legacy class), delayed startup metadata,
  same-kind changes, deduplication and removal of old tokens.
- The Metal interaction test switches palettes with the format menu open and
  asserts unchanged viewer diagnostics. Optional captures include `06-light`,
  `07-dark`, `08-hc-dark`, `09-hc-light` and `10-custom`; HC light and the custom
  palette with light editor/dark panels were visually inspected.

Live graphical verification used an isolated VS Code development profile and
Chrome via the DevTools protocol, without changing the user's editor settings.
With `picorv32.vtr` open and 29 signals added, a signal was selected, the cursor
placed at about 3.41 µs, a marker added, and the viewport zoomed. Theme changes through
Dark Modern, Default High Contrast, Default High Contrast Light and Light Modern
all repainted the viewer. Light Modern colour customizations then set an ivory
editor (`#fff4e6`), dark panels (`#203040`), blue status bar (`#005fb8`) and teal
waves (`#006b66`); removing those customizations restored the original colours.
Every switch retained the identical webview document and canvas objects. After converting the captures' embedded display profile to sRGB, pixel sampling
also confirmed the actual canvas background in all five palettes. The
captures retain the trace, selection, cursor, marker and zoom; HC outlines and
foregrounds were visually inspected. The initial light viewer capture also
uses the host palette. A standalone Chrome page still loads the sample in
One Dark. Local artifacts are `/tmp/volna-theme-shots/` (not committed), with
`vscode-{light,dark,hc-dark,hc-light,light-modern,custom,custom-removed}.png`
and `web-standalone.png` alongside the Metal captures.

To repeat: build the web bundle, launch `code --user-data-dir=<temporary-dir>
--extensionDevelopmentPath=<absolute vscode-ext path> <sample.vtr>`, select
Volna with **Reopen Editor With…** if the first launch selected the binary text
editor, add signals and interaction state, then change themes and
`workbench.colorCustomizations` in that temporary profile. Re-run the Metal
capture command above for reproducible automated state assertions.

Limits: the live run exercised bundled themes and customizations, not a matrix
of third-party theme extensions, VS Code versions or GPU backends. Screenshots
are spot checks, not a frame-by-frame startup recording or pixel-baseline test;
the no-default-flash guarantee comes from installing the palette before GPUI
window creation. Fonts/metrics remain bundled and do not follow VS Code font
settings. Contrast safeguards do not constitute a complete accessibility audit.
The pre-existing dependency `block` reports a future-Rust compatibility warning.

## Typed palette refinement (2026-09-12)

- `check.sh` passes strict native Clippy, formatting, 27 Rust tests, the Metal
  interaction test, and five JavaScript tests. The ignored timing test remains
  excluded. Four real VS Code 1.137 palettes now accompany synthetic and lifecycle
  tests; JavaScript verifies that generated Rust fixtures match the host mapping.
- Strict WASM Clippy passes with `cargo clippy -p volna --locked --target
  wasm32-unknown-unknown --lib --all-features -- -D warnings` (Homebrew LLVM 20
  configured for zstd-sys). The three new WASM warnings were fixed.
- `web/build.sh` builds the optimized WASM and extension media. No dependencies,
  VTR APIs, file format or decode paths changed.
- Tests cover supplied surface/state pairs, missing/invisible foregrounds,
  translucent chart RGB preservation, lightness-only stroke repair, opaque marker
  chips and labels, synchronous initial metadata, bounded 250 ms fallback and
  late metadata. State tests retain source/history identity and interaction state.
- Native captures under `/tmp/volna-refined-shots/` include the open format menu
  through all synthetic and four real palettes. Light, HC light and mixed-surface
  captures were inspected. Live VS Code 1.137 switches through both ordinary and
  both HC modes, custom colours and removal retained identical canvas/document
  objects and visible trace, selection, cursor, marker and zoom. Captures are in
  `/tmp/volna-theme-shots/`. Standalone Chrome still loads in One Dark.

The visual pass caught SVGs disappearing when their explicit colour was omitted:
GPUI SVGs do not inherit unset colour. The icon wrapper now explicitly reads the
inherited text colour, preserving icons while sharing row/button foregrounds.

Limits: missing metadata and alpha-chart edge cases have automated coverage, not
an exhaustive live host/browser matrix. Startup ordering is tested, but no
frame-by-frame recording proves absence of every startup flash. Third-party
extensions, other VS Code versions, Linux/Windows and other GPU backends were
not checked. The existing dependency `block` still emits its future-Rust warning.

After the final icon fix, a fresh isolated VS Code profile loaded the rebuilt
bundle and repeated all six switches successfully with the same document/canvas,
29 signals, a selected row, cursor at 7.835 µs, marker and zoom. The older reused
development profile failed to load a cached module (`tokens` redeclaration);
this did not reproduce in the fresh profile. Rebuilding files under a running
development host therefore remains a verification caveat. Standalone was also
reloaded against the final bundle and its icons visually checked.
