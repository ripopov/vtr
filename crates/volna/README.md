# Volna — the VTR/VDB viewer

Volna is the official viewer for VTR and its separate VDB design/presentation
companion, built with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
(the `gpui-pre` crates.io snapshot, version 0.3.4). It runs as a native macOS
app and, compiled to WebAssembly, inside a VS Code webview.

The current implementation displays **VTR waveforms**. VDB attachment, source
browsing, and transaction/pipeline views are planned. Keep static VDB semantics
separate from runtime VTR data as these features are developed.

Panels: a hierarchy browser (scope tree plus a separate, filterable variable
list) and a waveform panel with three pixel-aligned columns (names, values,
waves), a timeline, a cursor and numbered markers.

## Development checks

Rust 1.96 or newer is the supported toolchain baseline. Run `./check.sh` from
this directory (or `./crates/volna/check.sh` from the repository root) to check
formatting, deny Clippy warnings across all targets/features, and run tests.
The native build requires the platform SDK, including Metal tools on macOS.
Run `cargo fmt -p volna` to apply the committed formatting policy.
Volna uses the root workspace lockfile. Root commands without `-p` build the core
crates; use `-p volna` for the viewer or `--workspace` to include everything.

## Build and run (native)

```sh
cd crates/volna
cargo run -p volna --profile viewer -- examples/picorv32.vtr
cargo run -p volna --profile viewer -- --synthetic 100000000
cargo test -p volna
```

## Build and run (WebAssembly)

Requires Node.js, the `wasm32-unknown-unknown` target, `wasm-bindgen-cli`
matching the locked `wasm-bindgen` version, and a clang
that can target wasm32 for the `zstd` C sources (Apple's clang cannot; the
script picks up Homebrew LLVM automatically, or set `CC_wasm32_unknown_unknown`).

```sh
./web/build.sh                                     # → web/dist and vscode-ext/media
python3 -m http.server 8080                        # from crates/volna/
open "http://localhost:8080/web/?file=../examples/picorv32.vtr"
```

The page uses WebGPU when available and falls back to WebGL2. It is a
single-threaded build (no SharedArrayBuffer requirement), so it also runs where
cross-origin isolation is unavailable, such as VS Code webviews.

## VS Code extension

```sh
./web/build.sh
code --extensionDevelopmentPath="$PWD/vscode-ext" "$PWD/examples"
```

Opening any `*.vtr` file uses the viewer as a custom editor; the command
"Volna: Open Waveform Viewer" opens an empty viewer whose *Open* button
goes through VS Code's file dialog. Package with `npx @vscode/vsce package`
inside `vscode-ext/`.

## Using the viewer

| Action | Mouse | Keys |
|---|---|---|
| Add variables | double-click a variable; `+` in the Variables header adds all listed | `⏎` adds the selected variables |
| Search all variables | type in the filter with no scope selected | |
| Set cursor | click or drag in the waves or the timeline (snaps to nearby edges) | `shift-←/→` previous/next edge of the selected signal |
| Zoom | `⌘`/`ctrl` + wheel, pinch | `=` / `-`, `F` fit |
| Pan | horizontal wheel/trackpad, `shift` + wheel, middle/right drag | `←` `→`, `Home`/`S` start, `End`/`E` end, `C` centre on cursor |
| Markers | click a chip to jump, `shift`-click to remove | `M` add at cursor, `shift-M` clear |
| Value format | click the badge in the values column | `T` cycles binary / hex / decimal / signed / float |
| Rows | click, `shift`/`⌘` multi-select, drag the column dividers | `↑` `↓`, `⌫` remove, `⌘A`, `esc` |
| Sidebar | drag the dividers | `⌘B` toggle |
| Files | drag a `.vtr` onto the window | `⌘O` |

The status bar shows the trace range, cursor time, pixel resolution and the
smoothed paint time of the wave table.

## Verification and performance

```sh
cargo run -p volna --profile viewer --features visual-test --bin volna-visual -- --out results
```

renders the app offscreen with GPUI's Metal headless renderer, drives it with
real input events, saves screenshots to `results/` and prints frame times for
synthetic traces of 10 K, 1 M and 100 M transitions. See
[ARCHITECTURE.md](ARCHITECTURE.md) for how rendering cost is kept proportional
to the viewport width, and [VERIFICATION.md](VERIFICATION.md) for the recorded
results.

Bundled fonts and icons retain their [third-party licenses](THIRD_PARTY.md).
