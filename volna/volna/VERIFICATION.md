# Volna verification guide

Run checks from the repository root unless a command specifies otherwise.
Workspace development requires Rust 1.96+. Native GPUI builds require the
platform SDK; the macOS Metal interaction test also requires desktop services.

## Automated checks

```sh
volna/volna/check.sh
```

The script checks formatting and strict Clippy for `volna-core`, `volna` and
`volna-egui`, runs their Rust tests, and runs the Node tests for the VS Code
adapter. The frame-time benchmark is excluded from the regular test run.

For focused checks:

```sh
cargo test --locked -p volna-core
cargo test --locked -p volna-core --test fst
cargo test --locked -p volna-core --test transactions
cargo test --locked -p volna --all-features
cargo test --locked -p volna-egui --test screenshots
node --test volna/volna/vscode-ext/theme.test.mjs
```

| Area | Coverage |
|---|---|
| Document and loading | Latest open wins, close invalidates pending results, stale successes/errors are ignored, failed loads can retry, and duplicate/alias rows share histories |
| Headless interaction | Cursor, markers, selection, deterministic zoom/pan/fit, dense-column rendering, format menu, sidebar filtering/keys, layout hit regions and repaint coalescing |
| FST input | Plain/gzip fixtures, raw bytes, reals, nine-state values, aliases, EVCD payloads, event occurrences, unavailable samples and explicit unsupported metadata errors |
| FST/VTR parity | Values at every change timestamp in the committed Verilator features, operators and pipeline recordings |
| Transactions | Unsupported versus empty capabilities, typed attributes and phases, events, stages, parents, inclusive overlap boundaries, filtering, early stopping and cross-stream relations |
| GPUI adapter | Production loading executor, input events, theme changes and nonblank Metal frames |
| egui adapter | Production loading executor, VTR/FST opening, sidebar/divider dragging, filtering, selection, zoom/pan/fit and software-rendered screenshots |
| Host theme | Raw VS Code palettes, host-neutral CSS parsing, synchronous initial snapshot and subsequent theme updates |

FST fixture regeneration commands are in
[`fst_values.c`](../volna-core/tests/fixtures/fst_values.c). Regular tests use
committed fixtures. Framing checks reject truncated headers/payloads,
unfinished lengths and arithmetic overflow; they are not exhaustive validation
of malicious compressed payloads. Gzip wrappers are decompressed into memory,
so their memory use differs from native streaming-file access.

## Web and VS Code builds

```sh
volna/volna/web/build.sh
cargo check --locked -p volna-core --target wasm32-unknown-unknown
cargo clippy --locked -p volna --target wasm32-unknown-unknown --lib --all-features -- -D warnings
```

The build script selects a wasm-capable LLVM toolchain for zstd-sys, builds the
WASM library, runs wasm-bindgen, optionally runs wasm-opt, and copies the bundle
and asset licenses into the extension. Standalone Cargo commands require the
same `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown` settings; on
macOS, use a wasm-capable Homebrew LLVM toolchain rather than Apple's clang.
Generated web and extension media are ignored by Git. Compilation does not
replace browser runtime checks.

## Visual and interaction checks

Save native captures with an absolute output path:

```sh
VOLNA_SCREENSHOTS="$PWD/target/volna-screenshots" volna/volna/check.sh
```

The GPUI test renders offscreen through Metal with real input events and
asserts nonblank frames. It runs on the macOS main thread; other platforms
report a skip. The egui test rasterizes meshes in software without a window or
GPU and writes captures under `volna/volna-egui/results/egui/`. Neither renderer
uses pixel-baseline comparisons, and the software rasterizer does not test GPU
feathering differences.

For browser checks, build the bundle, serve `volna/volna` locally, and open
`web/?file=../examples/picorv32.vtr`. For VS Code, start an Extension Development
Host with a temporary `--user-data-dir` and
`--extensionDevelopmentPath=<absolute vscode-ext path>`, then open the bundled
trace with Volna. [`tools/cdp.mjs`](tools/cdp.mjs) supports browser input and
capture through the debugging protocol; `debug_state()` exposes viewer state.

Exercise the following in each target frontend:

- Load a trace, select scopes, filter variables, add signals and select rows.
- Move the cursor, zoom/pan/fit, navigate edges, and add/jump/remove markers.
- Open the format menu, switch translators and drag the column/sidebar dividers.
- Open another recording and verify that pending results cannot populate it.
- Check empty/loading states, text at 1× and 2×, and nonblank waveform rendering.

For VS Code theming, test Dark Modern, Light Modern, both high-contrast themes,
and mixed `workbench.colorCustomizations`. Change and remove customizations
with signals, cursor, selection, markers and an open format menu; the document
state must survive. Standalone web checks should also cover `volnaHostTheme()`,
`set_theme(json)`, malformed palettes and palettes without an explicit kind.
Host color safeguards do not constitute a complete accessibility audit.

## Performance checks

```sh
cargo test -p volna --profile viewer --features visual-test --test viewer frame_times -- --ignored
```

The ignored harness measures synthetic traces with 10 K, 1 M and 100 M
transitions. It uses a 1440×900 logical viewport, ten displayed signals and a
keyboard pan per frame. Compare fit-to-view and zoomed-in costs, recording the
hardware, toolchain and display scale with the results. `table_paint_ms` covers
the wave table; whole-frame time includes layout, sidebar and GPU submission.
There are no pass/fail timing thresholds.

Whole-history loading must be measured separately from painting. FST histories
store owned values and can use substantially more memory than the compressed
file. Batched queries and viewport-sized drawing do not establish bounded
loading memory or remote-file support. See
[ARCHITECTURE.md](ARCHITECTURE.md) for the query and rendering boundaries.
