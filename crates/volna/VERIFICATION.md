# Verification record

## Automated checks (2026-09-12)

- `crates/volna/check.sh`: formatting, strict Clippy across all targets/features,
  and all 21 tests pass against the main workspace's VTR crate.
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

- native: GPUI's Metal headless renderer (`cargo run -p volna --profile viewer --features visual-test --bin volna-visual`),
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
cargo run -p volna --profile viewer --features visual-test --bin volna-visual -- --out results
```

## Acceptance check (look and feel)

All colours are Zed One Dark tokens (`src/theme.rs`); fonts are IBM Plex Sans
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
