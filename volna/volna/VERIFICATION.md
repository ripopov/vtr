# Volna verification guide

Run checks from the repository root unless a command specifies otherwise.
Workspace development uses the dated nightly in `rust-toolchain.toml`, including
the WASM target, rustfmt and Clippy. Native GPUI builds require the
platform SDK; the macOS Metal interaction test also requires desktop services.
The `vtr-query` wire checks require the Protocol Buffers compiler (`protoc` on
PATH, or `PROTOC` pointing to it); native-only query builds do not use it.

## Automated checks

```sh
volna/volna/check.sh
```

The script checks formatting and strict Clippy for `vtr-query`, `vtr-server`, `volna-core`,
`volna` and `volna-egui`, runs their Rust tests, and runs the Node tests for the VS Code
adapter. The frame-time benchmark is excluded from the regular test run.

For focused checks:

```sh
cargo test --locked -p vtr-query --all-features
cargo test --locked -p vtr-server
cargo check --locked -p vtr-query --all-features --target wasm32-unknown-unknown
cargo test --locked -p volna-core
cargo test --locked -p volna-core --test fst
cargo test --locked -p volna-core --test transactions
cargo test --locked -p volna --all-features
cargo test --locked -p volna-egui --test screenshots
node --test volna/volna/vscode-ext/theme.test.mjs volna/volna/vscode-ext/workspace.test.cjs volna/volna/vscode-ext/relay.test.cjs
cargo build --locked -p vtr-server
# Use the Cargo target directory if CARGO_TARGET_DIR overrides ./target.
VTR_SERVER="$PWD/target/debug/vtr-server" node --test volna/volna/vscode-ext/relay-server.test.cjs
```

| Area | Coverage |
|---|---|
| Bounded-query foundation | Exact maximum-time endpoints, canonical grids across pans, half-open transaction/point boundaries, concurrent allocation admission and shared pins, fragmented/coalesced stdio frames, truncation, invalid lengths and terminal parser failures, exact/cold-summary and metadata pages, codec conformance and shared native delivery; this does not verify a live relay |
| Query worker and stdio child | Asynchronous opening, nonblocking future polling/drop, cancellation and abandoned-result cleanup, fixed delivery credit; real child handshake, paging/retries, release, operation limits, stale snapshots, malformed/version errors, EOF and close |
| RPC client | Prepaid reply capacity, transmission-order request IDs, priority cancellation, late/replaced-host replies, cross-page coverage, shared retries, control saturation, reentrant consumer wakeups and idle transport wakeups; production client versus real child across all current query families, plus the core multi-row demand scheduler and display list through that same RPC path |
| Extension relay module | Opaque exact ArrayBuffers, fragmented/coalesced frames, byte/slot admission, explicit consumption acknowledgement, stdin backpressure, hidden views, stale incarnations, truncated/unread output, shutdown escalation and a real native child handshake/error exchange. The module is not yet wired to the viewer; these are Node transport tests, not VS Code end-to-end verification. |
| Summary demand | Real native worker pages reach the core summary renderer; superseded delayed completions are drained, only the latest intent runs, repeated navigation releases operation capacity, failed intents can be retried or replaced, eight rows share a bounded operation pool, and identical demands share partial and complete bins. Row removal preserves the shared session; changing signals clears incompatible bins. Query snapshots connect it to the row model/painter; runtime hosts have not switched loading paths. |
| Query-backed rows | Native/RPC snapshots render in the existing panel/value-column painter, document generations and dataset IDs gate installation, late full-history results do not replace query data, partial exact snapshots remain immutable, and fractional/overscrolled projections stay aligned. Runtime host opening is not yet switched. |
| Query cursor values | Exact and summary lookup through native/RPC demand slots, ambiguous-bin rejection, raw nine-state/byte/IEEE preservation, VTR defaults, invalid packing and expanded display-size admission. |
| Exact coverage/rendering | Native and RPC pages assemble into the same bounded window; repeated timestamps spanning pages remain unproven until the boundary completes, rejected/foreign pages preserve the prefix, retries do not duplicate changes, and event/max-time rendering preserves raw semantics. The shared demand scheduler runs exact and summary rows, including representation switches, exact cancellation, duplicate consumers and bounded retries. Immutable query results also render through the existing row painter; runtime host adoption remains pending. |
| Summary rendering | Raw-bin display lists preserve aggregate identity when zoomed, leave coverage gaps unpainted, distinguish repeated event occurrences and project maximum-time single ticks. This renderer is used by query-backed row snapshots; runtime hosts still use full-history loading. |
| Document and loading | Latest open wins, close invalidates pending results, stale successes/errors are ignored, failed loads can retry, and duplicate/alias rows share histories |
| Panel model | Literal hierarchy paths and duplicate-name ambiguity; split/close/focus/layout validation; 5,000 generated command sequences; shared and independent navigation; cross-panel load reuse and stale pointer rejection |
| Headless interaction | Cursor, markers, selection, deterministic zoom/pan/fit, dense-column rendering, format menu, sidebar filtering/keys, layout hit regions and repaint coalescing |
| FST input | Plain/gzip fixtures, raw bytes, reals, nine-state values, aliases, EVCD payloads, event occurrences, unavailable samples and explicit unsupported metadata errors |
| FST/VTR parity | Values at every change timestamp in the committed Verilator features, operators and pipeline recordings |
| Transactions | Unsupported versus empty capabilities, typed attributes and phases, events, stages, parents, inclusive overlap boundaries, filtering, early stopping and cross-stream relations |
| GPUI adapter | Production loading executor, input filtering/Escape, keyboard popup selection/dismissal, theme changes and nonblank Metal frames |
| egui adapter | Production loading executor, VTR/FST opening, sidebar/divider dragging, filtering, selection, zoom/pan/fit and software-rendered screenshots |
| Workspace codec and lifecycle | Exact integer times, unknown panels/formats, unresolved locators, atomic prepare/commit, malformed and oversized inputs, idle revisions, ticket races, fallback precedence, Save As and transition flush failures |
| Workspace hosts | Native copied-trace idle save/reopen, atomic I/O and preference errors; opaque VS Code candidates, remote URI schemes, ticket destinations, write errors, hide/dispose sequencing and disabled storage |
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
Use `target` to select the Volna iframe and `context: "!!document.querySelector('canvas')"`
to select its content frame. `assert` fails unless its JavaScript predicate is true.
The driver fails on uncaught runtime exceptions as well as failed assertions.
Inspect the rendered panels in screenshots, not only restored model counts.
Activate the editor tab before screenshots: a hidden VS Code webview may defer
canvas frames. For persistence checks use a temporary copy of a trace, enable sidecar saving,
create splits/tabs and rows, wait past the idle interval, reload the VS Code
window, and assert all panels and loaded rows from `debug_state()`. Inspect the
sidecar to verify the split fractions, links and resource reference. Other
visual checks should set `volna.workspace.autosave` to `off`.

For the default-host compatibility check, do not pass `--enable-coi` or other
shared-memory flags to VS Code. In the Volna content frame, verify
`crossOriginIsolated === false` and `typeof SharedArrayBuffer === "undefined"`,
then load a trace and exercise filtering, signal loading, keyboard menus,
cursor and zoom. Use a fresh profile or clear cached webview resources so the
check runs the newly built bundle. Compilation alone does not establish that
the umbrella's compiled-in threading support is compatible with this host.

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
Each timed loop asserts at least one real canvas paint per keyboard frame;
a fresh-window split also verifies deferred installation and keyboard focus.
Set `VOLNA_PERF_PANELS=4` for four linked panels in a balanced split layout.
Compare the one-panel run against a detached worktree of the previous commit,
using the same toolchain, profile, machine and session. There are no automatic
pass/fail timing thresholds; investigate regressions before accepting results.

The workspace comparison uses Apple M5, macOS 26.6.2, pinned
`nightly-2026-04-14`, the `viewer` profile and a 1440×900 offscreen Metal window at 2× scale.
The baseline is `cdb6945`, with only the same paint counter/assertions added.
Three alternating baseline/one-panel/four-panel runs use 30 keyboard frames per
case with persistence disabled. Values below are the best run's median in ms;
[all medians, p90s and available table-paint samples](benchmarks/workspaces.csv)
are retained. These are painting/navigation measurements, not trace read/write
throughput or browser frame times.

| Transitions | View | Baseline, 1 panel | Workspace, 1 panel | Workspace, 4 panels |
|---|---|---:|---:|---:|
| 10 K | Fit | 1.09 | 1.13 | 1.31 |
| 10 K | Zoomed | 1.09 | 1.14 | 1.22 |
| 1 M | Fit | 0.60 | 0.65 | 1.59 |
| 1 M | Zoomed | 0.59 | 0.64 | 1.54 |
| 100 M | Fit | 0.93 | 0.98 | 1.99 |
| 100 M | Zoomed | 0.93 | 0.97 | 1.97 |

The single-panel cost is 0.04–0.05 ms (about 4–8%). This intentionally relaxes
the original proposal's zero-regression condition to retain one stock dock
path. Explicit panel invalidation avoids rebuilding dock chrome on each pan;
no performance improvement is claimed. Four linked panels remain below 2 ms
in these best-of-three cases, with smaller canvases and shared histories.

Whole-history loading must be measured separately from painting. FST histories
store owned values and can use substantially more memory than the compressed
file. Batched queries and viewport-sized drawing do not establish bounded
loading memory or remote-file support. See
[ARCHITECTURE.md](ARCHITECTURE.md) for the query and rendering boundaries.
