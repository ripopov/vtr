# VTR — Versatile Trace Record

VTR records hardware simulation waveforms, transactions, hierarchy, clocks,
relations, and structured logs. This repository contains the trace libraries,
conversion tools, a C++20 SDK, and Verilator integration. The
[Volna viewer](https://github.com/ripopov/volna) consumes VTR through a pinned
submodule and is developed separately.

| Package | Purpose |
|---|---|
| [vtr](vtr/README.md) | Rust format implementation, reader, writer, recovery, and activity indexes |
| [vtr-cpp](vtr-cpp/README.md) | Independent C++20 reader/writer and SystemVerilog/DPI helpers |
| [vtr-guard](vtr-guard/README.md) | Linux crash and stop guard |
| [vtr-cli](vtr-cli/README.md) | Inspection, queries, indexing, and conversion |
| [vtr-convert](vtr-convert/README.md) | FST, Kanata, and VCD importers |
| [vtr-convert-wasm](vtr-convert-wasm/README.md) | Browser conversion host |
| [vtr-progress](vtr-progress/README.md) | Conversion progress reporting |

Recordings use VTR 3.2, including scalar-page and shared-text encodings.
The C++ package is version 4.0;
activity sidecars have their own VTI 3.0 format. Older trace assets must be
regenerated. Rust and C++ implementations are independent and tested in both
writer/reader directions.

## Development

Use the pinned Rust toolchain and a C++20 compiler. Rust libraries and tests
build without initializing Verilator:

```sh
cargo test --workspace --locked
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

Build and test the independent native implementation with CMake; see the
[C++ guide](vtr-cpp/README.md). The native library does not link Rust.

The standalone `vtr` crate requires Rust 1.97.1 or newer. See the
[format specification](vtr/docs/SPEC.md), [design rationale](vtr/docs/RATIONALE.md),
and [Verilator integration](integrations/verilator/README.md).

## License

Licensed under MIT; see [LICENSE](LICENSE).
