# Shared VTR and VDB foundations

VTR stores runtime traces; VDB supplies separate design metadata and domain
semantics. Their libraries share a directory and a workspace, while retaining
the trace/design boundary defined in [GOAL.md](../GOAL.md).

| Package | Responsibility |
|---|---|
| [vtr](vtr/) | Rust trace reader/writer: waveforms, transactions, runtime hierarchy, relations and logs |
| [vtr-capi](vtr-capi/) | C ABI with public C/C++ headers in `include/` |
| [vtr-vdb](vtr-vdb/) | Separate RTL design database, netlist rendering, temporal driver tracing and the existing `vtr-vdb` CLI |

Static design semantics and presentation belong to VDB, never the VTR format.
VDB's intended scope includes RTL, ESL, pipeline and transaction debugging;
its current implementation provides the RTL companion. The standalone
[slang exporter](../integrations/slang/README.md) and
[Verilator integration](../integrations/verilator/README.md) produce that
metadata. Trace inspection/conversion lives under [tools/](../tools/README.md).

Run from the repository root:

```sh
cargo build --release
cargo test
cargo clippy --release
cargo test -p vtr-vdb
```

The default workspace commands also cover the CLI and Rust benchmark drivers,
without requiring the viewer's platform SDK. See the
[format specification](../docs/SPEC.md), [Rust API](../docs/API_RUST.md),
[C API header](vtr-capi/include/vtr.h), [VDB schema](../docs/VDB_RTL.md) and
[VDB application note](../docs/VDB_APPNOTE.md).
