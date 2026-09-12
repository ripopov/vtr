# Volna egui frontend

A native desktop frontend for Volna built on egui/eframe. Every viewer decision
lives in `volna-core`; this crate owns the window, paints the core's display
list with `egui::Painter`, hosts egui widgets for the chrome and runs the core's
loads on threads. There is no wasm build and no VS Code integration.

Open VTR or FST using the command line, file dialog, or drag and drop. Both
formats use the same core and background load loop. See the
[FST support notes](../volna/README.md) for supported values and limitations.

```sh
cargo run -p volna-egui --profile viewer -- volna/volna/examples/picorv32.vtr
cargo run -p volna-egui --profile viewer -- --synthetic 100000000
cargo test -p volna-egui          # headless interaction test with PNGs in results/egui/
```

`src/headless.rs` drives the app without a window and rasterizes egui's
tessellated output in software, so the screenshot test runs on any platform
without a GPU. Shortcuts and behaviour match the GPUI frontend; see
[../volna/README.md](../volna/README.md) and
[../volna/ARCHITECTURE.md](../volna/ARCHITECTURE.md).
