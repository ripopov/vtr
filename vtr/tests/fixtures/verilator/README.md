# Verilator log fixtures

These small binary VTR files are historical output of the prototype Verilator
`--trace-vtr` integration. They are reader regression inputs, not a simulator
integration included in this crate. Tests read them directly; no external tools
or submodules are needed.

Copied from `examples/verilator` in the prototype's Surfer fork, revision
`cdcdfc85686b151953e1c6fa006ffe0591269754`. The original fixture generator is
[`integrations/verilator/logs/run.py`](https://github.com/ripopov/vtr/blob/4b4fd692e7e03a7ebea4daf7a89787203eed9cb6/integrations/verilator/logs/run.py).

| File | Recorded scenario |
| --- | --- |
| `logs_normal.vtr` | Normal completion, severity mapping, partial writes |
| `logs_error.vtr` | `$error` at time 5 |
| `logs_fatal.vtr` | `$fatal` at time 5 |
| `logs_finish.vtr` | `$finish` at time 5 |
| `logs_bytes.vtr` | Recoverable non-UTF-8 output |
| `logs_runtime.vtr` | Runtime time-scale diagnostics |
| `logs_repeat.vtr` | Repeated-dump warnings |

Run `cargo test -p vtr --test verilator_logs`. Format changes must update these
fixtures and their assertions together. New Rust round-trip tests should create
traces in temporary directories rather than add generated binary fixtures.

The VTR fixtures use format 2.0. Their hierarchy declarations were re-encoded
with generator kinds and ordinary grouping scopes; clock generators replace
the former clock container and its edge source. Signal histories, transaction
IDs, timing, and log arguments are preserved.
