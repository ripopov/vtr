# VTR: Vibe Trace Record

VTR is an open trace file format and reference library for hardware
simulation. One file holds signal waveforms, transaction streams, the
elaborated design hierarchy and the runtime relations between transactions:
an open FSDB-class trace store that covers everything FST, FTR (LWTR4SC),
Konata/Kanata pipeline logs and OpenTelemetry traces can express.

* Rust reference implementation (`crates/vtr`), C ABI (`crates/vtr-capi`,
  header `crates/vtr-capi/include/vtr.h`), command-line tools
  (`crates/vtr-cli`) and a benchmark suite (`crates/vtr-bench`, `bench/`).
* Smaller files, faster writing and faster reading than FST and FTR on the
  workloads in the benchmark report.
* Streaming writer with a background encoder thread; random-access,
  memory-mapped reader that never needs to read the whole file for a local
  query; crash-recoverable files.
* Verilator integration: `integrations/verilator` adds `--trace-vtr` to
  Verilator 5.050 next to `--trace-fst`, so a Verilated model dumps VTR
  directly (used by the benchmarks on a full openC910 CoreMark run).

## Documents

| document | content |
|---|---|
| [docs/SPEC.md](docs/SPEC.md) | file format specification (normative) |
| [docs/API_RUST.md](docs/API_RUST.md) | Rust API reference and tour |
| [docs/API_C.md](docs/API_C.md) | C API reference |
| [docs/KDB_APPNOTE.md](docs/KDB_APPNOTE.md) | designing a KDB (semantics/presentation layer) on top of VTR, with a worked Konata pipeline viewer and the RTL-debugger outline |
| [docs/BENCHMARKS.md](docs/BENCHMARKS.md) | benchmark methodology; results in [docs/BENCHMARK_RESULTS.md](docs/BENCHMARK_RESULTS.md) |
| [docs/RATIONALE.md](docs/RATIONALE.md) | design rationale: alternatives researched, what was borrowed and rejected |
| [docs/COVERAGE.md](docs/COVERAGE.md) | feature-by-feature coverage of FST, FTR, Kanata and OpenTelemetry |
| [docs/SOTA_REVIEW_2026.md](docs/SOTA_REVIEW_2026.md) | 2025-2026 literature on trace/columnar compression checked against measurements; ranked ideas for further gains |

## Building

Requirements: Rust 1.80+, a C compiler (for the vendored zstd and the C
tests). Everything builds from a clean checkout with submodules
(`git submodule update --init`); the submodules under `ext/` are only used
by the converters' tests and by the benchmarks.

```sh
cargo build --release            # library, C library (target/release/libvtr.{a,so}), vtr CLI, vtr-bench
cargo test                       # unit, round-trip, converter and C-ABI tests
```

## Quick start

```sh
# Convert existing traces
vtr convert sim.fst sim.vtr                 # FST (lossless hierarchy + values)
vtr convert sim.vcd sim.vtr                 # VCD
vtr convert pipeline.log pipeline.vtr       # Kanata / Konata log (.log, .log.gz)
vtr convert trace.ftr trace.vtr             # FTR (LWTR4SC)
vtr convert spans.json spans.vtr            # OpenTelemetry OTLP/JSON

# Export and cross-check
vtr2vcd trace.vtr trace.vcd                       # VTR -> VCD (also: vtr to-vcd)
vtr fst-to-vcd trace.fst ref.vcd                  # FST -> VCD through fst-reader
vtr vcd-compare ref.vcd trace.vcd                 # same value changes, in total and per signal?

# Inspect
vtr info sim.vtr
vtr hier sim.vtr --vars --depth 3
vtr value sim.vtr top.cpu.pc 123450
vtr changes sim.vtr top.cpu.pc --from 100000 --to 200000
vtr dump sim.vtr --to 5000                  # VCD-like text
vtr tx pipeline.vtr --stream cpu.thread0 --from 100 --to 200
vtr tx pipeline.vtr --id 42
```

Writing from Rust:

```rust
use vtr::*;
let mut w = Writer::create("out.vtr")?;
w.set_timescale(-9)?;
w.begin_scope("top", ScopeType::Module, "top");
let (_, clk) = w.add_var("clk", VarType::Wire, Direction::Input, SignalKind::Bits { width: 1, states: 4 });
let (_, data) = w.add_var("data", VarType::Reg, Direction::Implicit, SignalKind::Bits { width: 32, states: 4 });
w.end_scope()?;
for t in 0..1000u64 {
    w.set_time(t * 10)?;
    w.emit_bit(clk, (t & 1) as u8)?;
    if t % 4 == 0 { w.emit_u64(data, t * 3)?; }
}
w.close()?;

let r = Reader::open("out.vtr")?;
let sig = r.find_signal("top.data", '.').unwrap();
println!("{}", r.value_at(sig, 405)?.to_ascii());
```

Writing from C (see `docs/API_C.md`):

```c
vtr_writer *w = vtr_writer_create("out.vtr", NULL);
vtr_writer_begin_scope(w, "top", 0, NULL);
uint32_t node, clk;
vtr_writer_add_var(w, "clk", 16, 1, 0, 1, 4, &node, &clk);
vtr_writer_end_scope(w);
vtr_writer_set_time(w, 0); vtr_writer_emit_bit(w, clk, 0);
vtr_writer_set_time(w, 5); vtr_writer_emit_bit(w, clk, 1);
if (vtr_writer_close(w) != VTR_OK) fprintf(stderr, "%s\n", vtr_last_error());
```

## Benchmarks

`python3 bench/run.py all` builds the harnesses (GTKWave's original
`fstapi.c`, libfstwriter, LWTR4SC's FTR writer, wellen/fst-reader) and
Verilator with the VTR backend, prepares the workloads (including Verilator
runs of two real RTL designs: the RSA-256 core and the openC910 SoC running
CoreMark, both traced to FST and to VTR by the simulator itself) and
regenerates `docs/BENCHMARK_RESULTS.md`. See `docs/BENCHMARKS.md`.

## Repository layout

```
crates/vtr         core library: container, codecs, hierarchy, signal blocks, transaction blocks, writer, reader
crates/vtr-capi    C ABI (libvtr) + header + C smoke test
crates/vtr-cli     `vtr` tool and the converters (FST, VCD, Kanata, OTLP JSON, FTR)
crates/vtr-bench   workload generation and benchmark drivers
bench/             C/C++ harnesses (FST, FTR), Verilator workloads (rsa256, c910), orchestrator, results
docs/              specification, API references, application note, benchmark report, rationale
integrations/      Verilator --trace-vtr patch and backend
ext/               reference submodules (libfstwriter, LWTR4SC, Konata, wavepeek, pulp-c910)
```

## License

MIT OR Apache-2.0 for everything in this repository. The submodules keep
their own licenses.
