# Trace tools

[vtr-cli](vtr-cli/) provides the existing `vtr` and `vtr2vcd` commands: trace
inspection and conversion from FST, VCD, FTR, Kanata and OpenTelemetry JSON.
It uses the shared trace library in `core/vtr`.

Run from the repository root:

```sh
cargo build --release -p vtr-cli
cargo test -p vtr-cli
cargo run -p vtr-cli --bin vtr -- info trace.vtr
```

The `vtr-vdb` debugging CLI remains with its design library in `core/vtr-vdb`.
The planned wavepeek integration is described in
[the integration guide](../integrations/README.md).
