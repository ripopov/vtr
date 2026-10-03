# Trace tools

[vtr-cli](vtr-cli/) provides the existing `vtr` and `vtr2vcd` commands: trace
inspection and conversion from FST, VCD, FTR, Kanata and OpenTelemetry JSON.
It uses the shared trace library in `core/vtr`.

[volna-server](volna-server/) is the standalone executable that serves raw
complete recordings beside remote files. It uses
[volna-trace](../volna/volna-trace/) for recording access and protocol objects;
it does not depend on the viewer core or either frontend. Run its independent
build, process/protocol tests and dependency checks with
`python3 tools/check-volna-loading.py`.

Run from the repository root:

```sh
cargo build --release -p vtr-cli
cargo test -p vtr-cli
cargo run -p vtr-cli --bin vtr -- info trace.vtr
```

The `vtr-vdb` debugging CLI remains with its design library in `core/vtr-vdb`.
The planned wavepeek integration is described in
[the integration guide](../integrations/README.md).
