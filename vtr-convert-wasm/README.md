# VTR browser converter

`vtr-convert-wasm` runs the shared [`vtr-convert`](../vtr-convert/README.md)
importers in a disposable browser worker. It accepts FST, Kanata (`.kanata` and
`.log`), and VCD filenames, case-insensitively, including `.gz` forms. Output is
a complete, checksummed VTR image with LZ4 compression and deduplication disabled
to preserve recorded samples. Conversion failures return no image.

`convert_to_vtr` accepts owned input bytes and a progress callback. The WASM
`convert` export forwards versioned JSON snapshots to
`globalThis.volnaConversionProgress`. The worker host installs that callback,
imports the module, reads the selected `File`, calls the export, and transfers
the output buffer to the page. Terminating the worker cancels conversion even
while Rust is executing synchronously. Workers are terminated after success,
failure, cancellation, replacement, or page exit.

The converter has its own unshared WASM memory and does not depend on the viewer.
The viewer and trace loader consume only the resulting VTR image. The browser
keeps the source filename and URI for workspace persistence; converted bytes are
not written to disk or uploaded. The selected input, externally decompressed
gzip image, and encoded output are each limited to 256 MiB. `max_input_bytes`
lets the worker refuse oversized files before reading them. Nested gzip inputs
are rejected. The output sink also reports allocation failures without publishing
an image.

The browser build caps converter WASM memory at 1 GiB, including retained FST
histories and Kanata instructions. A small compressed file can still exceed that
ceiling during conversion; the worker reports a failure and releases its memory.
These are converter limits, not a total browser memory budget: file buffers,
transfer buffers, and the viewer need additional RAM. Use the native app or CLI
for larger recordings.

From `docs/`, `npm run build:viewer` builds both WASM modules with the pinned Rust
toolchain and matching `wasm-bindgen` CLI. The converter uses the installed
`wasm32-unknown-unknown` target and no shared-memory flags. Generated assets live
under `docs/public/viewer/converter/` and are excluded from Git. A revalidated
manifest selects content-versioned JavaScript and WASM filenames, which can use
normal HTTP caching. The manifest and converter assets are requested only when
an import starts; startup and direct VTR opening do not request them.

```sh
cargo test --locked -p vtr-convert-wasm -p vtr-progress -p vtr-convert
cargo clippy --locked -p vtr-convert-wasm -p vtr-progress -p vtr-convert --all-targets -- -D warnings
cd docs
npm run build
npm run test:imports
```

Native tests compare memory output against the same file writer byte-for-byte,
check gzip equivalence, progress completion, and failure behavior. Browser tests
exercise the compiled WASM worker through the actual viewer host, including
pipeline and waveform loading, cancellation, replacement, unavailable WASM,
malformed input, memory refusal, direct VTR opens, lazy asset requests, download
retry, and workspace identity.
