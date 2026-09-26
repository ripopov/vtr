# Volna verification guide

Run checks from the repository root unless a command specifies otherwise.
Workspace development uses the dated nightly in `rust-toolchain.toml`, including
the WASM target, rustfmt and Clippy. Native GPUI builds require the
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
cargo test --locked -p volna-core --test pipeline       # pipeline panel: open, load, paint, zoom, sync, save/restore
cargo test --locked -p volna-core --test table_baseline # reduced table: sources, bounds, identity, copy, details, restore
cargo test --locked -p volna-core --test settings      # settings.json store, edits, search, the Settings tab
cargo test --locked -p volna --all-features            # includes the manifest check and the settings render test
cargo test --locked -p volna-egui --test screenshots
node --test volna/volna/vscode-ext/theme.test.mjs volna/volna/vscode-ext/workspace.test.cjs
cargo test --locked -p volna-server
cargo clippy --locked -p volna-server --all-targets -- -D warnings
node --test volna/volna/vscode-ext/trace-host.test.cjs
node --test volna/volna/tools/table-clipboard.test.mjs
```

| Area | Coverage |
|---|---|
| Document and loading | Latest open wins, close invalidates pending results, stale successes/errors are ignored, failed loads can retry through the row menu without adding rows or replacing unrelated histories, duplicate/alias rows share histories, queued demand is removed with its last consumer, and active signal loads survive removal/re-add |
| Panel model | Literal hierarchy paths and duplicate-name ambiguity; split/close/focus/layout validation; 5,000 generated command sequences; shared and independent navigation; cross-panel load reuse and stale pointer rejection |
| Headless interaction | Cursor, markers, selection, deterministic zoom/pan/fit, dense-column rendering, value-format menu, signal-name context menu over one row or a selected group, drag-to-reorder rows (click threshold, groups, no-op drops, Escape, edge auto-scroll), sidebar filtering/keys, layout hit regions and repaint coalescing |
| FST input | Plain/gzip fixtures, raw bytes, reals, nine-state values, aliases, EVCD payloads, event occurrences, unavailable samples and explicit unsupported metadata errors |
| FST/VTR parity | Values at every change timestamp in the committed Verilator features, operators and pipeline recordings |
| Transactions | Unsupported versus empty capabilities, typed attributes and phases, events, stages, parents, inclusive overlap boundaries, filtering, early stopping and cross-stream relations |
| Pipeline panel | Enter on a stream or generator opens a panel without a notice and queues one track load; a second panel on a resident track queues nothing; one quad per visible primary-lane stage and one band per overlay; unmodified zoom about the pointer keeps the time and row under it at interface zoom 1 and 2; Ctrl/Cmd-wheel zooms time only and preserves rows and activity following; linked wheel zoom moves the wave panel, unlinked does not; trackpad deltas pan; a click sets the shared cursor to the integer cycle and the wave value column follows; density steps bound painted rows by pixels; flushed and open rows differ by colour; workspace round trip, unresolved tracks and invalid saved rows; closing the last panel releases the track and a late delivery is ignored; failed loads retry; the checked-in showcase opens both cores |
| Analog waves | Translator numeric readings, limits and label values; reals open as linear 3× plots; `A` toggles the plottable selection and restores 1×; format-menu Draw and Range sections and the signal-menu toggle; trace, window and type ranges with window easing; zoomed-out columns keep a one-sample glitch and X breaks the line; exact samples with dots and the cursor dot; hover readouts of samples and dense columns; row-edge resizing to presets; version-3 workspace rows; summaries built on the worker, charged to the budget, waited for zoomed out, and released with the plot; summary queries against scans at every block boundary |
| Transaction lanes | Add to Waves on generators only; one shared resident generator with pipelines, released with the last lane; default heights from stacking depth (capped at 4×); bar hits on the right sub-row, snapping to record ends, empty-space clearing and Show Transaction; edge steps through every begin and end; cut/paste across panels, Height menu, `+N` fold chip and hatching; typed workspace rows with unresolved generators; captions, value column, red failures and the density strip; stacking, boundaries and density bins against scans |
| Baseline table panel | Generator and fixed-signal sources; shared raw ownership and native/remote admission; distinct merged timestamps and one-signal zero-axis path; bounded 256-row preparation; exact `u64` navigation and scrollbar endpoints; fixed columns; stable selection/cursor linking; details limits and cancellation; 64 KiB complete TSV; visible-row accessibility; loading/cancel/retry/refusal; versioned workspace restore at row one; browser clipboard rejection with selectable TSV and Retry |
| GPUI adapter | Production loading executor, input filtering/Escape, keyboard popup selection/dismissal, theme changes and nonblank Metal frames |
| egui adapter | Production loading executor, VTR/FST opening, sidebar/divider dragging, filtering, selection, zoom/pan/fit and software-rendered screenshots |
| Workspace codec and lifecycle | Exact integer times, unknown panels/formats, unresolved locators, atomic prepare/commit, malformed and oversized inputs, idle revisions, ticket races, fallback precedence, Save As and transition flush failures |
| Workspace hosts | Native copied-trace idle save/reopen, atomic I/O and preference errors; opaque VS Code candidates, remote URI schemes, ticket destinations, write errors, hide/dispose sequencing and disabled storage |
| Host theme | Raw VS Code palettes, host-neutral CSS parsing, synchronous initial snapshot and subsequent theme updates |
| Complete-object transport | Framing and compression limits, cooperative metadata/history/track decoding, admission accounting, atomic installation, stale identities, disconnect and per-item errors; real child-process VTR/FST equivalence and track round trips |

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
With default sidebar/column widths, rows loaded and the editor focused,
`tools/verify-navigation.mjs <port>`
checks standalone browser navigation through real input and exported viewport
state. For VS Code, append the active webview URL fragment and its content-frame
origin in CSS pixels, for example `id=<webview-id> 52 90`. Use a temporary trace
copy with autosave off. The verifier checks range gestures, Control/Command,
Escape, repeated zoom, pan destinations, wheel zoom, cursor zoom, endpoints,
page keys, Shift-wheel isolation and middle/right-drag, and writes a final screenshot under `/tmp`.
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
- Exercise repeated animated pans and zooms; PageUp/PageDown; Shift-Z with a
  cursor offscreen; wheel pan over waves versus row scrolling over names;
  Ctrl/Cmd-wheel and pinch anchored at the pointer; and right-drag panning.
- Ctrl/Cmd-left-drag a time range in both directions, including vertical drift.
  Inspect the preview before release and the resulting time bounds afterward.
  Check that small/vertical-only drags do not zoom, Escape cancels selection,
  middle/right-drag pans, and linked versus independent panels behave correctly.
- Open the value-format menu and switch translators. Right-click a selected
  signal name, then open the selected group in a table and remove it; repeat
  with Shift+F10. Drag the column/sidebar dividers.
- Open another recording and verify that pending results cannot populate it.
- Check empty/loading states, text at 1× and 2×, and nonblank waveform rendering.

For VS Code theming, test Dark Modern, Light Modern, both high-contrast themes,
and mixed `workbench.colorCustomizations`. Change and remove customizations
with signals, cursor, selection, markers and an open format menu; the document
state must survive. Standalone web checks should also cover `volnaHostTheme()`,
`set_theme(json)`, malformed palettes and palettes without an explicit kind.
Host color safeguards do not constitute a complete accessibility audit.

### Local/remote VS Code image comparison

Build the baseline and current extension bundles in separate worktrees. Launch
each in an isolated Extension Development Host with a separate user-data directory
and `--remote-debugging-port` (for example, 9335 and 9334). Use the same VS Code
version, theme, display backend and scale; `--ozone-platform=x11` selects X11 on
Linux. Disable workspace autosave in both test profiles. Open each worktree's
`volna/volna/examples/picorv32.vtr`, dismiss host notifications, and load the eight variables
in the root `testbench` scope. Clear the variable filter. Set both viewers to the
same cursor, viewport, selection and layout before comparing.

For a 1200×800 window at device scale 1, with the default sidebar layout:

```sh
# Python requires Pillow; Node must be >= 22.
python3 volna/volna/tools/compare-remote-ui.py 9335 9334 /tmp/volna-ui-comparison
```

The tool verifies eight loaded rows and exact frame geometry, captures the
starting pose, applies identical cursor/zoom/pan actions and keyboard filtering,
and compares viewer pixels. It also asserts zero outgoing remote messages during
those actions. It saves scripts, console logs, images and numerical results for
inspection. The comparison excludes VS Code breadcrumbs (different paths) and
the live status counter; it does not mask waveform or sidebar content. Inspect
the images as well as the equality result to verify the intended visible state.

The [retained comparison](benchmarks/client-server-ui/results.json) against
`d56358bfcfd0aafc550416cfc715a0fbcd65e827` has zero differing pixels in all three
poses and zero remote messages. The [baseline](benchmarks/client-server-ui/baseline-filtered.png)
and [remote](benchmarks/client-server-ui/current-filtered.png) captures show the
same cursor values and filtered waveform view. This covers a small RTL recording
in one theme and scale. It does not establish large-object performance, other
themes/scales, or transaction rendering. Animation-frame samples from background
windows are throttled and must not be used to claim frame-time parity.

## Pipeline panel checks

`cargo test -p volna-core --test pipeline` builds a small PIPELINE trace and
exercises the headless checks listed above; two tests open checked-in
recordings: `volna/volna/examples/pipeline_showcase.vtr` and
`pipeline_demo.vtr`, which the Verilator pipeline tracer suite writes (see the
examples README; its stream counts in the declared clock and a click feeds
the Transaction panel). The
macOS harness (`cargo test -p volna --features visual-test --test viewer`) opens
that trace, activates both `pipeline` streams from the sidebar, asserts two
ready panels of more than 500 rows, wheel-zooms about a point, clicks to set
the cursor and saves `pipeline-two-cores` and `pipeline-zoomed-cursor`. Inspect
them for: the cycle axis and `cycle` unit in every panel, stage cells in the
pipeline-order ladder colours (hues by each name's mean position in a row;
neighbours alternate in lightness beyond eight names) with names once rows are tall enough, grey
stall bands over the lower part of rows, red-tinted flushed rows with a tick in
the label column, the dashed edge of open rows, the shared cursor chip at the
same cycle in the wave and pipeline panels, and the hover text in the status
bar (`#row · stage [begin, end) · label`).

In each frontend (native, standalone web, VS Code), open the showcase: the
start panel lists the trace extent, `0 variables`, its transaction tracks and
both PIPELINE streams as buttons. Double click `soc.cpu0.pipeline` (or press
its button): the pipeline panel takes the start panel's place and no waveform
panel appears. Then: wheel over the cells (both axes zoom about the
pointer, the wave panel follows; in a browser every wheel is a precise scroll,
so hold Ctrl/⌘ to zoom time only), two-finger scroll (both axes pan), Shift+wheel
(time only), left drag (pans; a short press sets the cursor), `↑ ↓`, `= -`,
`F`, `L` then wheel again (the wave panel stays), `M`, drag the label divider,
split the panel (`⌘\`, the copy shows the same track without a new load),
close panels (closing the last one brings the start panel back; `⌘W` on it
closes the trace), and restore a saved workspace with a pipeline panel. Rows
below two pixels must stay responsive over ten thousand rows. The harness
saves `start-panel` after closing both pipeline panels.

Large pipelines are measured, not gated:

```sh
cargo test --release -p volna-core --test pipeline half_million -- --ignored --nocapture
VOLNA_OBJECT_MIB=2048 cargo test --release -p volna-core --test pipeline half_million -- --ignored --nocapture
VOLNA_OBJECT_MIB=2048 VOLNA_PIPELINE_TRACE=$PWD/volna/volna/examples/c910_coremark.vtr \
  VOLNA_PIPELINE_STREAM=TX.core0.pipeline \
  cargo test --release -p volna-core --test pipeline half_million -- --ignored --nocapture
```

The test writes half a million C910-shaped instructions (fourteen stage
names, a fifth of them aborted) or opens the given capture, then prints load,
first-frame and per-frame layout-plus-paint times over the whole trace, a
5,000-cycle and a 150-cycle window, and the process's resident memory. On an
Apple M5: at the default 256 MiB object limit the load fails explicitly (the
generator is 541 MiB, 699 MiB for the 411,339-row C910 capture with its fetch
stages) and the panel offers its retry; with the limit raised the stream loads
in 0.36 s (0.39 s for the capture), resident memory grows by 1 to 1.4 GB, and a frame takes 14 ms
over the whole trace, 0.15 ms over 5,000 cycles and 0.01 ms over 150.

## Hierarchy browser checks

`cargo test -p volna-core --test hierarchy` creates a mixed VTR containing RTL,
pipeline and unknown-kind streams, generators, enum references and log provenance.
It checks search ordering/caps, notices without transaction loads, leaf navigation,
saved stream paths, malformed identities and deep trees. Cooperative metadata
tests cover the same raw fields through fragmented protocol decoding. Rebuild
both host and client for protocol version 2.

To exercise and capture the mixed browser in the macOS offscreen Metal harness:

```sh
VOLNA_HIERARCHY_FIXTURE=/tmp/volna-hierarchy.vtr \
  cargo test -p volna-core --test hierarchy -- --test-threads=1
VOLNA_WORKSPACE=off VOLNA_HIERARCHY_FIXTURE=/tmp/volna-hierarchy.vtr \
  VOLNA_SCREENSHOTS=/tmp/volna-hierarchy-shots \
  cargo test -p volna --features visual-test --test viewer
```

Inspect `hierarchy-variables`, `hierarchy-pipeline`, `hierarchy-log` and
`hierarchy-search`: glyphs, port direction, unknown stream tags, log badge/location,
breadcrumb and full result paths. Check Enter and double-click on a stream or
generator show a visible notice; plus and unselected Enter add only variables.
Chevron clicks must not change selection. Tab switches sidebar panes, Down from
the filter selects the first result, and Escape clears selection then filter.
Check narrow sidebars, long names, light/dark/high-contrast palettes and the
5,000-result indicator. egui needs only its existing waveform actions and readable
shared member rows. Native, standalone web and VS Code use the same core model.

## Performance checks

### Shared loading baseline

The loading comparison starts from the complete-object implementation immediately
before the unification refactor, including the existing uncommitted work. It does
not compare against the older whole-file VS Code implementation. The
[environment manifest](benchmarks/loading-unification/environment.json) records
fixture, executable and restored-source hashes. The
[baseline patch](benchmarks/loading-unification/baseline.patch) restores that
runtime implementation from this revision while keeping an identical native
complete-track measurement entry point. Apply it only in an isolated source copy;
build baseline and current with separate Cargo target directories.

Measurements use an Intel Core Ultra 7 265K, pinned `nightly-2026-04-14`, the
`viewer` profile and cores 0–7. The runner alternates order and retains one warmup
plus seven fresh-process samples per variant/workload. Record counts and errors
must match. CPU time is the process family's user plus system CPU, measured with
`getrusage(RUSAGE_CHILDREN)`; it includes startup, cleanup and the waited-for server
child, unlike the loader's internal wall-clock timer. Peak RSS is Linux `VmHWM`.

Generate the smaller transaction fixture and build the measurement programs:

```sh
cargo build --release -p vtr-bench
target/release/vtr-bench gen-tlm /tmp/volna-tlm-10k.txr 10000
target/release/vtr-bench tx-write /tmp/volna-tlm-10k.txr /tmp/volna-tlm-10k.vtr --no-background
cargo build --locked -p volna-core -p volna-server --profile viewer \
  --example load_cost --example remote_cost --bin volna-server
```

RSA, Kanata and large TLM fixture generation is documented in `docs/BENCHMARKS.md`.
The baseline patch changes no VTR reader, encoding or file-format code. The track
round-trip test verifies that borrowed serialization emits the same bytes as the
owned wire payload, including typed details and large values.

### Native complete-history loading

`load_cost` opens through the shared session backend, retains complete histories
in batches of 64 canonical signals, or loads every complete stream with `tracks`.
The latter includes relation extraction and client index construction, replacing
the former benchmark's record-only transaction query. Both variants use the same
measurement source and retain their results through the RSS sample.

```sh
python3 volna/volna/tools/bench-loading.py \
  /tmp/volna-dry-baseline-target/viewer/examples/load_cost \
  target/viewer/examples/load_cost /tmp/native-load.jsonl \
  --samples 7 --transaction-trace /tmp/volna-tlm-10k.vtr
```

[All native samples](benchmarks/loading-unification/native.jsonl): values below
are baseline / current. RSS comes from the fastest total-time sample; CPU uses
the median of all measured samples.

| Selection | Best total, ms | Median total, ms | Peak RSS, KiB | Median process CPU, ms |
|---|---:|---:|---:|---:|
| PicoRV32 VTR, 64 signals | 0.288 / 0.284 | 0.294 / 0.292 | 3,508 / 3,476 | 1.339 / 1.297 |
| RSA VTR, 64 signals | 17.959 / 17.779 | 19.082 / 18.468 | 51,948 / 51,928 | 21.756 / 21.350 |
| RSA FST, 64 signals | 118.566 / 118.580 | 121.397 / 119.469 | 129,420 / 129,340 | 146.338 / 143.674 |
| TLM, 30,000 transactions, complete streams | 36.655 / 36.547 | 36.949 / 37.797 | 41,460 / 41,464 | 45.854 / 46.538 |

Native costs remain comparable. The TLM median increases 2.3% while its fastest
sample is slightly faster; these small variations do not establish a speedup or
regression. Native histories still share the reader's immutable storage and do
not pass through serialization or a server process.

### Complete-object child protocol

`remote_cost` drives the real child with the production `RemoteClient`, retaining
completed objects and measuring framing, compression, admission, validation and
index construction. It reports first-ready latency, CPU decoder work, bytes in
both directions and per-process peak RSS. It does not simulate browser scheduling.

```sh
python3 volna/volna/tools/bench-loading.py \
  /tmp/volna-dry-baseline/binaries/remote_cost target/viewer/examples/remote_cost \
  /tmp/remote-load.jsonl --samples 7 \
  --remote-servers /tmp/volna-dry-baseline/binaries/volna-server target/viewer/volna-server \
  --transaction-trace /tmp/volna-tlm-10k.vtr
```

[All remote samples](benchmarks/loading-unification/remote.jsonl), baseline /
current. Times and RSS use the fastest total-time sample; CPU uses the median.

| Selection | First ready, ms | Total, ms | Server peak RSS, KiB | Median process-family CPU, ms |
|---|---:|---:|---:|---:|
| PicoRV32 VTR, 64 signals | 1.530 / 1.349 | 6.016 / 5.770 | 3,708 / 3,644 | 7.230 / 7.117 |
| RSA VTR, 64 signals | 44.373 / 44.087 | 678.955 / 676.478 | 84,596 / 84,612 | 664.495 / 665.754 |
| RSA FST, 64 signals | 165.712 / 159.860 | 792.904 / 787.247 | 159,220 / 159,228 | 771.918 / 772.536 |
| TLM, 30,000 transactions | 24.935 / 23.250 | 172.787 / 165.300 | 30,560 / 25,916 | 171.441 / 163.001 |
| Kanata, 4,041 transactions | 81.892 / 72.506 | 81.896 / 72.510 | 38,864 / 24,988 | 89.042 / 78.861 |

Waveform costs remain comparable. Borrowing track records for serialization
reduces server copies: TLM server RSS falls 15%, and Kanata falls 36%; total times
fall about 4% and 11%, respectively. Client peak RSS remains comparable (about
29,600 KiB for TLM and 17,900 KiB for Kanata; exact values
are in the samples). This is a server allocation improvement, not a smaller
client representation.

The current best samples send 103,685 bytes for PicoRV32, 23,810,628 for RSA VTR,
23,810,692 for RSA FST, 2,088,418 for TLM and 1,047,379 for Kanata. Corresponding
client bytes are 14,399, 27,105, 27,105, 4,781 and 1,906. Both variants have the
same payload representation, counts and frame sequence. Small compressed-size
variations reflect fresh random session identities in the envelopes.

The retained [initial native](benchmarks/loading-unification/initial-native.jsonl)
and [initial remote](benchmarks/loading-unification/initial-remote.jsonl) cohorts
precede the final queue consolidation. PicoRV32 remote wall-clock outliers occur
in both variants despite comparable CPU work. The final seven-sample cohort has
median baseline/current totals of 6.206/6.028 ms; the apparent slowdown does not
recur. No sample was removed from either cohort.

### Limits and large tracks

Run the real process path with explicit memory/object limits in MiB:

```sh
target/viewer/examples/remote_cost target/viewer/volna-server \
  bench/results/latest/rsa256.vtr signals:64 512 1
target/viewer/examples/remote_cost target/viewer/volna-server \
  bench/results/latest/c910_coremark.vtr signals:0 1 256
target/viewer/examples/remote_cost target/viewer/volna-server \
  bench/results/latest/tlm_1m.vtr tracks 512 256
```

[Paired limit samples](benchmarks/loading-unification/limits.jsonl) return
identical records and errors. The signal limit refuses three histories while
61 succeed. The metadata-budget failure releases admission storage. Three
alternating large-track runs retain 501,070 transactions and 862,771 incident
relation entries, with explicit errors for seven tracks. Incident entries can
include the same edge in both endpoint generators.

| Large-track metric | Baseline | Current |
|---|---:|---:|
| Total time, range | 15.13–15.28 s | 13.74–13.82 s |
| Best first-ready time | 2.671 s | 2.487 s |
| Median process-family CPU | 14.789 s | 13.359 s |
| Server peak RSS, best sample | 2,755,956 KiB | 2,185,992 KiB |
| Client peak RSS, best sample | 553,744 KiB | 553,692 KiB |
| Decoded admission peak | 536,869,896 bytes | 536,869,896 bytes |

Server peak RSS falls about 21%, and total time about 9%. Backpressure and
client admission remain unchanged. The 512 MiB admission budget does not cap
process RSS or the server reader's decoded caches. Per-process high-water marks
are not a simultaneously sampled combined memory total. These checks do not
claim that arbitrary complete selections fit the defaults.

### Browser responsiveness and visual comparison

Use isolated VS Code development hosts with the baseline/current extensions,
`--ozone-platform=x11 --disable-gpu-vsync --force-device-scale-factor=1`, a
1200×800 content viewport and DPR 1. The editor iframe must be at (48,92), size
1152×686. The graphics flag avoids an inherited test-host vsync stall; it is not
a product setting or an application performance fix. Keep the tested window
in the foreground and do not run CPU benchmarks concurrently.

```sh
python3 volna/volna/tools/compare-cold-ui.py 9342 9343 /tmp/loading-cold-ui
python3 volna/volna/tools/profile-remote-ui.py 9343 /tmp/loading-slow-ui --send-delay-ms 50
```

The matched comparison reopens the RSA webview for each sample, selects all 27
signals in `TOP.Testbench.i_rsa.i_RSAMont`, then observes three seconds. OS caches
remain warm; startup and metadata are excluded. The
[six samples](benchmarks/loading-unification/gui/samples.json) have median gaps
of 16.5–16.6 ms in both variants, p95 gaps of 17.5–17.6 ms, and no observed tasks
over 50 ms. Each sample has one isolated frame gap over 50 ms (baseline maximum
76.6 ms, current 61.5 ms). Thus there is no measured frame-time regression, but
also no universal 60 Hz or maximum-gap guarantee.

All three [image pairs](benchmarks/loading-unification/gui/visual.json) are
pixel-identical. The captures were inspected to verify that they show the same
27 loaded waveforms. The [final normal-bundle follow-up](benchmarks/loading-unification/gui-final/samples.json)
also loads all rows and is [pixel-identical](benchmarks/loading-unification/gui-final/visual.json).
It records p95 gaps of 17.6/17.1 ms and maximum gaps of 38.8/54.7 ms for
baseline/current, with no long tasks. The isolated current gap remains within
the range observed in the repeated baseline cohort. The six-run cohort precedes
the final command-identity-exhaustion failure-path fix; the follow-up uses the
final bundle, whose hash is recorded in the environment manifest.

The [delayed transfer](benchmarks/loading-unification/slow-ui.json) completes
in 13.83 s with 23,704,758 incoming bytes. Cursor/zoom input takes effect while
only four rows are ready; all 27 subsequently load. Across 835 animation callbacks
the maximum gap is 17.8 ms, with no observed task over 50 ms. Loaded navigation
sends zero packets. The warm WASM heap grows from 59.6 to 85.3 MB; linear-memory
capacity is not process RSS and need not shrink when previous objects are freed.

A 50 ms alarm in the profiling scripts is an investigation trigger. Compare
matched baseline runs before attributing an isolated frame gap to the loader.
To capture renderer and GPU work alongside a generated replay script:

```sh
node volna/volna/tools/trace-cdp.mjs 9343 \
  /tmp/loading-slow-ui/profile.json /tmp/loading-trace.json
```

Standalone Chrome also exercises the real HTML file input for VTR and FST.
The [VTR capture](benchmarks/loading-unification/gui/standalone-vtr.png) shows
all eight root signals loaded, and the [FST capture](benchmarks/loading-unification/gui/standalone-fst.png)
shows all five. This path calls `open_trace` with file bytes and has no remote
host callbacks. The CDP helper's `files` action drives the actual file input and
its change handler, using absolute fixture paths.

### Browser transaction loading

The pipeline panel is the production consumer of complete tracks. The opt-in
`remote-profile` diagnostic consumer still exercises document ownership,
transport, decoder and indexes without a panel:

```sh
VOLNA_WEB_FEATURES=remote-profile volna/volna/web/build.sh
# Open /tmp/volna-tlm-10k.vtr in the isolated development host first.
python3 volna/volna/tools/profile-remote-tracks.py 9343 \
  /tmp/volna-tlm-10k.vtr /tmp/loading-tracks
# Rebuild the normal bundle after profiling:
volna/volna/web/build.sh
```

Use the wasm-bindgen CLI version matching Cargo.lock. The diagnostic feature is
absent from normal bundles. Its consumer retains each raw stream, reports counts,
then releases them. The host yields with MessageChannel tasks between slices of
bounded decoder steps, targeting 2 ms per slice. This target is not a hard upper
bound for every allocation or index operation.


The [actual browser sample](benchmarks/loading-unification/browser-tracks.json)
loads all nine streams: 30,000 transactions and 54,328 incident relation entries,
with no errors. Total observation is 224.9 ms; first-ready observation is 100.7 ms
(polls every 100 ms). It receives 2,087,827 bytes. Across 31 animation callbacks,
the maximum gap is 17.1 ms, with no tasks over 50 ms during loading or release.
Release takes 10.2 ms including polling, leaves no retained streams, and sends
zero packets. WASM linear memory grows from 8.98 to 26.02 MB and retains that
capacity after release. This is a current-path responsiveness check, not a
matched browser transaction speedup claim. The larger paired process benchmark
above measures the server allocation improvement separately.

### Transaction lanes

```sh
cargo run --release -p volna-core --example lane_cost -- 1000000
```

`lane_cost` writes a synthetic generator of N overlapping reads (address and
data stages, 1% failures, depth 10), opens it through the shared session
backend with lanes, and prints the lane load time, the generator index build
(which includes sub-row stacking), and best-of-five core frames (layout and
`Scene` painting at 1400×600, `MonoMeasure`) with bars over 2,000 time units,
the same bars folded to 1×, and the whole trace as a density strip. Results
and the A/B against the build without lanes are in `docs/RATIONALE.md`
("Volna transaction lanes").

### Signal histories

```sh
taskset -c 0-7 cargo run --release -p volna-core --example history_cost -- \
  volna/volna/examples/large_fst.fst clk sine_100m
```

`history_cost` opens an FST or VTR trace, loads the named signals in one
batch and prints load time, RSS growth, counted bytes, random point queries,
1400-column frame sweeps (whole signal and 1/100 of it), a sequential time
scan and random and sequential numeric decodes. Run one configuration per
process. Results are in `docs/RATIONALE.md` ("Volna FST signal histories").

### Analog waves

```sh
cargo run --release -p volna-core --example analog_cost -- 10000000
```

`analog_cost` writes a 16-bit signed bus and a real signal changing every
tick, opens them through the shared session backend, times the block summary
build, and prints best-of-five core frames (layout and `Scene` painting at
1400×300, `MonoMeasure`) with the bus drawn digitally and as step and linear
plots in each range, zoomed out, at 1/100 of the trace and over 200 ticks.
Results are in `docs/RATIONALE.md` ("Volna analog waves").

### Painting

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
