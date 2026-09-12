# Timestamped HDL log capture

`run.py` builds the local Verilator backend in one- and two-thread modes and
checks eight scenarios: ordinary output, `$error`, `$fatal`, `$finish`, binary
`%c` output, `$printtimescale`, repeated-dump warnings, and a different current
context while opening the trace. Every scenario also opens the trace twice to
verify that an already-open trace does not register another sink.

Build the release VTR library/CLI and the pinned Verilator first, then run:

```sh
python3 integrations/verilator/logs/run.py --verilator <prefix>/bin/verilator
```

Add `--update-examples` to regenerate the small recordings in
`ext/surfer/examples/verilator/logs_*.vtr`. These are real simulator outputs.
The root Rust test `cargo test -p vtr --test verilator_logs` checks their
single-stream/six-generator schema, zero-duration transaction projection,
severity, timestamps, binary-byte preservation and shutdown readability.

The separate `simulation_logs` Rust example creates the larger synthetic
viewer workload. See [LOGGING.md](../../../docs/LOGGING.md#8-verilator-and-surfer)
for commands and [the Surfer chapter](../../../ext/surfer/docs/html/simulation-logs.html)
for UI behavior and memory ownership.

## Coverage and limits

The simulator checks validate text, severity, timestamps, sink ownership and
shutdown readability in both thread modes. The committed-fixture Rust test
checks the resulting trace schema independently of a local Verilator build.
These targeted checks do not replace Verilator's upstream regression suite.

The Surfer log tile virtualizes visible rows and filters in a worker. Its
per-recording index retains all formatted log text, so UI virtualization does
not imply bounded memory while opening a recording. Use the scale test in the
[Surfer log chapter](../../../ext/surfer/docs/html/simulation-logs.html) to
measure indexing, filtering and frame costs on the target machine.
