# AGENTS.md

Instructions for coding agents working in this repository. The project's
purpose, scope, requirements and ground rules are in [GOAL.md](GOAL.md);
read it first and treat its section 8 as binding.

## What this is

VTR (Vibe Trace Record): a trace file format and reference library for
hardware simulation traces (waveforms, transactions, hierarchy, relations).
Rust workspace with four crates (`crates/vtr` core, `vtr-capi` C ABI,
`vtr-cli` tools, `vtr-bench` benchmarks), a Verilator backend in
`integrations/verilator`, and a benchmark suite in `bench/`.

## Where to look

| need | file |
|---|---|
| file format (normative) | `docs/SPEC.md` |
| why things are the way they are, what was tried and rejected | `docs/RATIONALE.md` |
| APIs | `docs/API_RUST.md`, `docs/API_C.md` |
| benchmark method / current numbers | `docs/BENCHMARKS.md`, `docs/BENCHMARK_RESULTS.md` |
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
  a round-trip test in `crates/vtr/tests/`, and a rationale entry. Never
  break reading of existing files.
- **Simulator-integrated numbers** depend on the Verilator models linked
  against `libvtr.a`; their Makefiles do not track the library, so delete
  `bench/workloads/gen/*/obj_vtr/{Vtop,rsa_tb}` after changing the writer
  before running the suite.
- **Benchmarks are noisy at the few-percent level**: pin to P-cores
  (`taskset -c 0-7`), run best-of-N, and compare against a baseline binary
  in the same session.
- Keep the public API small and the C API a one-to-one projection of the
  Rust one. No presentation or KDB data in the format (GOAL.md section 2).
- Docs are part of the deliverable: update `SPEC.md`, `RATIONALE.md` and
  the API references in the same change as the code.
- Commit messages describe what changed and the measured effect; no
  attribution trailers.
