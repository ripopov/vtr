# Simulator and consumer integrations

This directory owns first-party glue between simulators/consumers and the
VTR trace and VDB design foundations. Simulator-independent implementation
belongs in `core/vtr` or `core/vtr-vdb`; viewer behavior belongs in `volna/volna-core`.
External projects and simulator forks remain pinned under `ext/`.

## Implemented workflows

The [Verilator integration](verilator/README.md) builds the pinned backend
with direct VTR tracing and VDB/source-index export. Its tests live beside
the build glue:

```sh
integrations/verilator/build.sh "$PWD/bench/build/verilator/install"
cargo build --release
python3 integrations/verilator/smoke/run.py --verilator bench/build/verilator/install/bin/verilator
python3 integrations/verilator/logs/run.py --verilator bench/build/verilator/install/bin/verilator
cargo build -p vtr-vdb
python3 integrations/verilator/vdb/run.py --verilator bench/build/verilator/install/bin/verilator
```

Consult the Verilator guide for prerequisites and build options. The standalone
slang exporter lives in [slang/](slang/README.md), with schema and usage in the
[VDB guide](../docs/VDB_RTL.md). The pinned [Surfer](../ext/surfer/) project is an
existing VTR/VDB consumer; Volna remains the official UI.

## Planned integrations

- **SystemC:** the current C ABI is usable from C++/SystemC and the converter
  reads FTR. Dedicated instrumentation should record runtime activity into
  VTR and keep static design semantics in VDB.
- **gem5:** native trace/design integration is planned. Kanata conversion and
  the Konata reference provide existing pipeline groundwork.
- **wavepeek:** the pinned reference under `ext/wavepeek` informs reader query
  requirements. A VTR/VDB adapter for AI-assisted debugging is planned.
- **Other simulators:** adapters should reuse the format/library boundaries
  and preserve trace/design identity without embedding VDB data in VTR.

These are integration directions, not placeholder implementations. Add an
adapter directory when it has code or a concrete integration workflow.
