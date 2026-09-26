# The vtr_trace package in other simulators

`vtr_trace_standalone.cpp` lets any simulator with DPI-C and VPI run the
`vtr_trace` SystemVerilog package: declared clocks
([vtr_clocks.html](../../docs/vtr_clocks.html)) and pipeline and other
trackers ([c910-verilator-tx-stream.html](../../docs/c910-verilator-tx-stream.html)).
The VTR Verilator fork does not need it: `--trace-vtr` ships the package and
writes its calls into the waveform file.

Compile these with the design and the tracer modules, and link `libvtr`
(`cargo build --release -p vtr-capi` builds `target/release/libvtr.a`):

- `core/vtr-capi/include/vtr_trace.sv`, before the files that import it;
- `integrations/systemverilog/vtr_trace_standalone.cpp`, with
  `core/vtr-capi/include` on the include path.

Run with `+vtr_trace=run.vtr`. The file opens at the first package call (a
constructor in an `initial` block), in the simulator's time precision, and
closes at the end-of-simulation callback, or at process exit when the
simulator never reports one. It holds the package's clocks, the trackers'
transactions and the package's warnings in a `simulation_log` stream, but no
signals. Without the plusarg every call does nothing and no file is written.

Instance paths come from `svGetScope`, time from `vpi_get_time`. Under
Verilator (`--vpi`), whose instance names start with `TOP`, `$root` means that
wrapper, and the harness forwards the start- and end-of-simulation callbacks
(`main.cpp`).

`run.py` is the check. It builds the pipeline tracer suite's design
(`integrations/verilator/pipeline`) with an upstream Verilator, which knows
nothing about VTR, in one and two thread modes, and requires exactly the
transactions the fork records, the declared clock, the package's warning and
no file without the plusarg:

```sh
cargo build --release -p vtr-capi -p vtr-cli
python3 integrations/systemverilog/run.py [--verilator verilator]
```

The `pipeline-tracing` workflow runs it with Ubuntu's Verilator.
