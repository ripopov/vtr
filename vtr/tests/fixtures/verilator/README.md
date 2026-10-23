# Verilator log fixtures

These VTR 3.2 files are direct output of the pinned Verilator fork at
`6eb99ee3ac238b0373707660aa7708122741933f`. The HDL and C++ driver live in
[`integrations/verilator/logs`](../../../../integrations/verilator/logs).
No format conversion or declaration rewriting is applied to the recordings.

Each scenario runs with both inline and threaded C++ writers:

| Scenario | Recorded behavior |
| --- | --- |
| `normal` | Severity metadata, timestamps, and separate partial writes |
| `error` | `$error` at time 5 with nonfatal error handling |
| `fatal` | `$fatal` at time 5 and trace finalization by the exit callback |
| `finish` | `$finish` at time 5 |
| `bytes` | Non-UTF-8 HDL output escaped without terminating recording |
| `runtime` | `$printtimescale` runtime output |
| `repeat` | A repeated-dump warning at time 3 |

From the repository root, after building the native library and Verilator:

```sh
python3 integrations/verilator/logs/run.py --verilator build/verilator/install/bin/verilator
cargo test -p vtr --test verilator_logs
```

The tests decode the public typed text payloads, check severity sources and
retained records, and fully validate every recording. Rust round-trip tests
create temporary recordings instead of adding generated fixtures here.
