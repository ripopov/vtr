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

## Validation on this checkout

- All 16 simulator runs passed their text, severity, timestamp and lifecycle
  assertions. Fatal traces finalized successfully. C++14 runtime syntax checking
  with `-Werror` passed.
- Four root fixture tests passed. The root workspace test suite passed; release
  Clippy completed with an existing `vtr-bench` unnecessary-cast warning.
- Surfer: 844 library tests passed, two ignored. The separately invoked scale
  benchmark passed. Clippy with `-D warnings` passed. Eight PNG baselines were
  rendered, visually reviewed and checked, including mouse-driven generator
  selection, time fields/cursor navigation, persisted queries and recording
  replacement. Documentation links and interactive transitions were checked;
  desktop/narrow browser inspection was unavailable because no browser was
  connected.
- With 1,010,000 indexed messages, the scale test rendered 27 rows per frame.
  The final measured mean CPU UI frame was 0.199 ms (100 frames), versus 0.198 ms
  with 2,020 messages. Initial open/index took 133.6 ms on the larger input;
  fuzzy filtering took 163.3 ms in a worker, with a maximum measured UI frame
  of 0.476 ms during that search. These are local development-profile results,
  not GPU or disk-cold guarantees. The index retains all formatted log text.
- The broad upstream Verilator run was **not green**: 3,627 passed, 153 failed,
  212 skipped. Missing `wavediff`, unavailable LZ4 include paths, installed-build
  path/golden mismatches, and sanitizer startup failures affected that run.
  Three stalled tests were terminated after stack samples showed recursive
  AddressSanitizer initialization before model entry. Two AST-dump regressions
  found during development were corrected and rerun successfully with the
  final compiler, along with display/merge and the new VTR code-generation
  tests (five targeted tests passed). Do not treat this as a clean upstream
  suite result. `make cppcheck` lacked cppcheck; the out-of-tree `make lint-py`
  target could not resolve its source-tree file list. Changed C++ files were
  formatted with clang-format 18.1.8.
