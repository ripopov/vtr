# AGENTS.md

Instructions for coding agents working in this repository. The project's
purpose, scope, requirements and ground rules are in [GOAL.md](GOAL.md);
read it first and treat its section 8 as binding.

## What this is

VTR (Vibe Trace Record): a trace file format and reference library for
hardware simulation traces (waveforms, transactions, hierarchy, relations).
Rust workspace with five crates (`crates/vtr` core, `vtr-capi` C ABI,
`vtr-cli` tools, `vtr-bench` benchmarks, and the `vtr-vdb` RTL companion),
a Verilator backend in the pinned `ext/verilator` submodule with build tools
in `integrations/verilator`, and a benchmark suite in `bench/`.

## Long-term RTL VDB goal

Generate VDB alongside Verilator builds from elaborated RTL before aggressive
optimization, preserving module hierarchy, specialized types, and source semantics.
Export an explicit mapping to traced signals and a shared VDB/VTR build identity
so debugging reflects the simulated design. Keep VDB separate from VTR, with a
simulator-independent schema and a slang adapter for standalone design browsing
and other simulation backends.

## Long-term VDB scope

Evolve VDB beyond RTL debugging to support Konata-like pipeline traces,
Gantt-chart-like transaction views, and other trace domains and visualizations.
Keep its format flexible, versioned, and extensible so new domain semantics
and presentation metadata can be added without forcing them into an RTL-only
model. VDB remains separate from the runtime trace data in VTR.

## Where to look

| need | file |
|---|---|
| file format (normative) | `docs/SPEC.md` |
| why things are the way they are, what was tried and rejected | `docs/RATIONALE.md` |
| APIs | `docs/API_RUST.md`, `docs/API_C.md` |
| logging (log sites, `LOG_BLOCK`, C++ header, comparison with NanoLog/binlog/Quill/CLP) | `docs/LOGGING.md`, `bench/log/` |
| benchmark method / current numbers | `docs/BENCHMARKS.md`, `docs/BENCHMARK_RESULTS.md` |
| RTL VDB schema, Verilator/pyslang exporters, Surfer attachment | `docs/VDB_RTL.md` |
| VDB source index (`verilator_vdb_index`, slang submodule of the Verilator fork) and Surfer's source tile | `integrations/verilator/README.md`, `docs/VDB_RTL.md`, `ext/surfer/docs/html/source-code.html` |
| open ideas ranked by measured headroom | `docs/SOTA_REVIEW_2026.md` |

## Build and test

```sh
cargo build --release          # library, libvtr.{a,so}, vtr CLI, vtr-bench
cargo test                     # unit, round-trip, converter and C-ABI tests
cargo clippy --release
python3 bench/run.py all --scale small   # quick benchmark (minutes)
python3 bench/run.py all                 # full suite (~1 h), regenerates docs/BENCHMARK_RESULTS.md
```

## Rules

- **Research first; design the best API.** This whole project is research.
  Backward compatibility of the Rust API, C API/ABI, and file format is not
  a requirement during this phase. Prefer the cleanest, most coherent API
  over preserving existing signatures, ownership choices, or behavior.
  Update affected callers, tests, and documentation together; keep the
  format versioned and extensible as required by GOAL.md section 8.
- **The reader targets read-only use.** Consumers inspect immutable trace
  data; modifying reader results is not a target use case. Design reader
  results for immutable access and shared storage where useful, including
  repeated requests for the same signal. Do not duplicate buffers merely
  to preserve independently mutable results. Consumers that need to
  transform data can explicitly create their own mutable copies.
- **Measure, don't assume.** Any change to encoding, compression, block or
  run sizes, transforms, or the reader's decode path must be A/B'd against a
  build of the previous commit (a `git worktree` of HEAD is the pattern) on
  the benchmark files, then confirmed with the full suite before the results
  are updated. Report size, write time and read time together; a change
  must not trade one for another.
- **Check `docs/RATIONALE.md` before re-trying an idea.** Most obvious
  alternatives have been measured; if you re-test one, record the new
  numbers there.
- **Format changes** need: a new code in `docs/SPEC.md`, reader support,
  a round-trip test in `crates/vtr/tests/`, and a rationale entry. Breaking
  changes must use an appropriate format version or code so incompatible
  files are rejected clearly; regenerate affected fixtures as needed.
- **Simulator-integrated numbers** depend on the Verilator models linked
  against `libvtr.a`; their Makefiles do not track the library, so delete
  `bench/workloads/gen/*/obj_vtr/{Vtop,rsa_tb}` after changing the writer
  before running the suite.
- **Benchmarks are noisy at the few-percent level**: pin to P-cores
  (`taskset -c 0-7`), run best-of-N, and compare against a baseline binary
  in the same session.
- Keep the public API small and the C API a one-to-one projection of the
  Rust one. No presentation or VDB data in the format (GOAL.md section 2).
- Docs are part of the deliverable: update `SPEC.md`, `RATIONALE.md` and
  the API references in the same change as the code.
- Commit messages describe what changed and the measured effect; no
  attribution trailers.
