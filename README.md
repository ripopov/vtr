# VTR hardware debugging environment

This repository builds a unified hardware debugging environment spanning RTL
and electronic system-level (ESL) models, including SystemC and gem5. VTR
stores runtime traces, VDB supplies separate design metadata and presentation,
simulator integrations connect them to Verilator and future backends, and
Volna is the official user interface. Planned wavepeek and MCP integrations
will let AI agents assist with investigations.

The [architecture guide](docs/ARCHITECTURE.md) describes component boundaries
and distinguishes implemented capabilities from plans. The requirements and
directions below are binding design goals, not claims that every item is
implemented today.

## VTR trace foundation

VTR (Volna Trace Record) is an open trace file format and reference library for
hardware simulation: an open FSDB-class trace store. One file contains signal
waveforms, transaction streams, elaborated runtime hierarchy, and runtime
relations between transactions. It also represents the simulator's text log as
typed, timestamped records; see [the logging guide](docs/LOGGING.md).

The current implementation provides:

- A Rust reference library (`core/vtr`), C ABI (`core/vtr-capi`), command-line
  tools (`tools/vtr-cli`), and benchmark suite (`bench/vtr-bench`, `bench/`).
- Smaller files, faster writing, and faster reading than FST and FTR on the
  workloads in the [benchmark report](docs/BENCHMARK_RESULTS.md).
- A streaming writer with a background encoder thread and a crash-recoverable
  format; a random-access, memory-mapped reader that answers local queries
  without reading the whole file.
- A pinned Verilator 5.050 backend in `ext/verilator`. The integration in
  `integrations/verilator` adds `--trace-vtr` beside `--trace-fst`; the
  benchmarks use it for a full openC910 CoreMark run.
- The `vtr_trace` SystemVerilog package: declared clocks and pipeline
  tracers bound to an unchanged design, which record instructions as
  transactions through a keyed tracker (`vtr_track.hpp`), in the fork's
  waveform file or, in other simulators, through a standalone sink. The
  openC910 tracer records every CoreMark instruction with its stages, folds
  and flushes; see [pipeline tracing](docs/c910-verilator-tx-stream.html).
- `verilator_vdb_index`, a slang-based source indexer in the pinned Verilator,
  and a Surfer source tile that uses its static VDB index for highlighting,
  cursor-time signal values, and navigation without starting another process.

VDB (Volna Data Base) is the separate design and presentation companion. The
`vtr-vdb` crate and CLI currently provide RTL netlists and temporal driver
tracing. [Volna](volna/volna/README.md), built with GPUI for native macOS and
the web/VS Code, currently displays VTR and FST waveforms; VDB attachment and
source/transaction views remain planned.

Explore the [Pipeline Studio UX demo](demos/pipeline-viewer/index.html) and its
[research and run instructions](demos/pipeline-viewer/README.md).

## Project requirements

These requirements define the VTR foundation. They deliberately do not
prescribe file layout, encoding, compression, indexing, or API shape; those
choices belong in the specification and rationale.

### What VTR captures

VTR records information that cannot be derived statically from source or
configuration, with the log-site exception described below:

1. **Runtime simulation traces:** signal value changes and transaction streams
   with timestamped events and attributes.
2. **Elaboration-time hierarchy:** scopes, instances, signals, streams, and
   their attributes as instantiated at runtime. This includes hierarchy built
   dynamically from inputs such as an XML platform description.
3. **Runtime semantic links:** parent/child and other relations across streams
   and hierarchy. For example, a CPU instruction can parent a NoC transaction,
   which parents activity in a slave device. FTR is the inspiration here.

### VTR/VDB boundary

The architecture follows Verdi's separation of waveform and design databases:

- **VTR, like FSDB,** contains trace data, runtime hierarchy, identity, and
  runtime relations.
- **VDB** is a separate, application-specific, CSS-like layer that adds source
  semantics and presentation to an unchanged VTR: colors, pipeline-stage
  meaning, squashed-instruction rules, source locations for signals/modules,
  specialized types, and driver/load annotations.

Many domain-specific VDBs may interpret the same VTR. Therefore VTR must not
embed presentation or design source semantics, but it must carry stable names,
identities, and attributes to which a VDB can bind across runs of one design.
No VDB format is part of the VTR format deliverable; its implementation remains
separate in `core/vtr-vdb/`. The sole source provenance exception is
`log.file`, `log.line`, and `log.func`, which identify where a runtime message
originated.

This pairing must support:

- A Konata-style pipeline viewer, with lifecycle and relations in VTR and
  coloring, flush meaning, and lane assignment in VDB.
- An RTL debugger with waveforms, source annotations, drivers, and reverse
  stepping.
- Automated and LLM-driven querying in the style of `ext/wavepeek`.

The same boundary applies to remote protocols: query and transport layers
carry raw trace data only; VDB profiles, presentation, and user annotations
remain client-side.

### Feature coverage

VTR must losslessly represent every feature supported by these sources; the
[coverage document](docs/COVERAGE.md) maps each feature to VTR:

1. **FST (GTKWave):** every scope and variable type; bit vectors with 4-state
   and 9-state values; reals, strings, enums, and integers; hierarchy
   attributes, timescale, time ranges, blackout/dump-off regions, aliases,
   writer and reader features, random access, and partial reads.
2. **FTR (LWTR4SC/SystemC):** streams, generators, transactions with begin/end
   times, typed attributes at begin/record/end, and relations across streams.
3. **Konata/Kanata:** instruction lifecycle, stages and lanes, retirement and
   flush, dependencies, labels, and per-cycle details.
4. **OpenTelemetry Tracing API:** traces, spans with start/end times, span
   kinds, attributes, events, links, status, parent/child and cross-trace
   relations, resources, and instrumentation scopes.

### Efficiency

VTR must beat FST and FTR in all meaningful scenarios:

1. **Space:** smaller equivalent trace files.
2. **Write speed:** lower simulator tracing overhead.
3. **Read and navigation speed:** faster open, hierarchy browsing, value-at-
   time lookup, window scans, and conditional event searches.

Meaningful scenarios include small and large RTL designs, long runs with few
active signals, short runs with many active signals, wide buses,
transaction-heavy SystemC/TLM workloads, and out-of-order pipeline traces.
Synthetic microbenchmarks alone are insufficient. Every efficiency claim must
be supported by the reproducible benchmark report.

### APIs

1. The reference implementation is Rust.
2. A one-to-one C ABI wrapper makes it usable from C, C++, and SystemC without
   requiring Rust toolchain knowledge.
3. The APIs must match or beat FSDB, FST, and FTR in clarity: a small surface,
   explicit lifetimes and ownership, no hidden global state, clear errors, a
   streaming writer, a random-access reader, and no whole-file read for a
   local query.
4. The writer must impose minimal overhead on a running simulator.

### Deliverables

The repository must contain and build all of the following:

1. A tested Rust reference implementation and C ABI wrapper.
2. A complete VTR file-format specification from which an independent
   implementation can be written.
3. Rust API documentation and a self-contained public C header.
4. A VDB application note with a worked Konata-like pipeline viewer and an RTL
   debugger outline.
5. A reproducible benchmark suite and report covering space, write time, and
   read time against FST and FTR on realistic workloads.
6. A design rationale recording researched alternatives and explaining the
   selected format and API decisions.

## Project direction

### VDB

Generate VDB alongside Verilator builds from elaborated RTL before aggressive
optimization. Preserve hierarchy, specialized types, and source semantics;
export an explicit traced-signal mapping and shared VDB/VTR build identity.
Keep the schema simulator-independent and retain a slang adapter for standalone
design browsing and other simulation backends.

VDB should grow beyond RTL to represent Konata-like pipelines,
Gantt-chart-style transaction views, and future trace domains. Keep it
versioned and extensible rather than forcing new semantics into an RTL-only
model or into VTR.

### Volna

Three intents guide Volna's evolution. They describe direction rather than a
fixed object tree; `volna/volna/ARCHITECTURE.md` records current behavior.

1. **Toolkit-independent core.** Document state, view models, viewport math,
   cursor and selection behavior, loading, and toolkit-neutral display lists
   belong in `volna-core`, with headless tests. Frontends paint and host native
   widgets; they do not own viewer logic.
2. **Several frontends over one core.** `volna` (GPUI, native and wasm) is the
   main viewer and feature target. `volna-egui` only proves core independence;
   preserve its current minimal behavior but do not pursue feature parity.
   Other frontends remain thin adapters. Dense data canvases are shared while
   chrome uses native toolkit widgets. Frontend-neutral saved state must be
   readable across frontends even when one cannot display every saved view.
3. **Client/server loading for remote files.** A reader beside a large remote
   VTR supplies complete metadata, complete selected signal histories, and
   complete selected transaction tracks to a VS Code remote client. Navigation,
   loaded-data search, summaries, analysis, and presentation remain client-side.
   Local and remote sessions yield the same immutable objects; local loading
   shares mapped buffers without serialization. Transfer and memory scale with
   complete selected data, not the visible window. Loading is asynchronous and
   fails explicitly at configured limits. Native local loading continues
   through the same session abstraction. See
   [the client/server design](docs/client-server-simple.html).

### Agent-driven debugging

Support MCP so an AI agent can query traces and launch or control Volna from an
existing conversation. Users should be able to select signals or time ranges
and add comments that steer the same investigation. An initial feedback path
may compose a message containing a stable selection snapshot and readable
trace, signal, and time references. MCP adapters must stay thin over
toolkit-independent core commands and query APIs and preserve the VTR/VDB
split. This is a long-term direction, not a claim of implemented support or a
requirement for embedded chat.

## Documentation

| Need | Document |
|---|---|
| Repository boundaries, layout, and integration status | [Architecture](docs/ARCHITECTURE.md), [integrations](integrations/README.md) |
| Normative file format | [Specification](docs/SPEC.md) |
| Rust and C APIs | [Rust API](docs/API_RUST.md), [C header](core/vtr-capi/include/vtr.h) |
| Design rationale and rejected alternatives | [Rationale](docs/RATIONALE.md) |
| Feature coverage | [Coverage](docs/COVERAGE.md) |
| Logging and comparisons with NanoLog/binlog/Quill/CLP | [Logging](docs/LOGGING.md), `bench/log/` |
| Benchmark method and current results | [Method](docs/BENCHMARKS.md), [results](docs/BENCHMARK_RESULTS.md) |
| VDB design and Konata application | [VDB application note](docs/VDB_APPNOTE.md), [Konata plan](docs/VDB_KONATA_PLAN.html) |
| RTL VDB schema, exporters, source index, and Surfer attachment | [RTL VDB](docs/VDB_RTL.md), [Verilator integration](integrations/verilator/README.md), `ext/surfer/docs/html/source-code.html` |
| Open optimization ideas | [State-of-the-art review](docs/SOTA_REVIEW_2026.md) |
| Volna architecture and verification | [Architecture](volna/volna/ARCHITECTURE.md), [guide](volna/volna/README.md), [verification](volna/volna/VERIFICATION.md), [egui](volna/volna-egui/README.md) |
| Panels, docking, and persistent workspace sessions (proposal) | [Workspaces](docs/workspaces.html) |
| `settings.json`, fuzzy-search editor, and VS Code parity (implemented design) | [Settings](docs/user-settings.html), [Volna architecture](volna/volna/ARCHITECTURE.md) |
| Mixed scope/stream hierarchy, member search, semantic icons, and log provenance (implemented; transaction panels deferred) | [Hierarchy](docs/hierarchy.html), [Volna architecture](volna/volna/ARCHITECTURE.md) |
| Konata-style pipeline rows with a shared time axis and map-like zoom (implemented) | [Pipeline](docs/pipeline-view.html), [follow activity demo](docs/follow-activity.html), [examples](volna/volna/examples/README.md), [Konata plan](docs/VDB_KONATA_PLAN.html) |
| General table panel over transactions, signal groups, logs, and synthetic rows (proposal) | [Full proposal](docs/table-panel.html), `docs/egui-table/` measured prototype, `docs/table/` superseded sketches |
| Reduced immutable single-generator table with bounded viewport work (proposal/demo) | [Baseline](docs/table-baseline.html) |
| Analog plots of buses and reals in the wave panel: step/linear drawing, trace/window/type ranges, glitch-preserving min/max columns (implemented) | [Analog waves](docs/analog-waves.html) |
| Transaction generator lanes in the wave panel with stacking, folding, density and shared selection (proposal/demo) | [Transaction lanes](docs/transaction-waveforms.html) |
| Shared transaction detail panel for table and pipeline selections: lifeline, typed attributes, stages, events, related records (proposal/demo) | [Transaction panel](docs/tx-detail.html) |
| Instruction pipeline tracing from Verilator into VTR with a reusable SystemVerilog tracer API; openC910 proof of concept (proposal/demo on recorded data) | [C910 pipeline tracing](docs/c910-verilator-tx-stream.html), `bench/workloads/c910/pipeline_replay.py` |
| Owner-declared clocks stored in VTR as steady stretches (first edge, last edge, period; DVFS and gating included) and their use in Volna as cycle rulers, cycle readouts, edge snapping and pipelines in cycles (proposal/demo) | [Clocks](docs/vtr_clocks.html) |
| Adding hierarchy during a run (UVM objects, initial blocks): how VTR hierarchy works, the reader fix for late signals, an ID-based tree API replacing the scope stack, caller migration and core tests (implemented) | [Dynamic hierarchy](docs/dyn_hierarchy.html) |

## Building and testing

Requirements are rustup, the dated nightly in `rust-toolchain.toml`, a C
compiler for vendored zstd and C tests, and the native platform SDK for Volna
(including Metal tools on macOS). The nightly is required by GPUI Kit's WASM
dependencies. Core crates retain their lower declared MSRVs; an explicit
stable toolchain needs Rust 1.96+ to include Volna's manifest.

Initialize all submodules from a clean checkout; the Verilator fork contains a
nested slang submodule. Other `ext/` projects support converter tests,
integration, research, and benchmarks.

```sh
git submodule update --init --recursive
cargo build --release          # library, libvtr.{a,so}, CLI, benchmark driver
cargo test                     # unit, round-trip, converter, and C-ABI tests
cargo clippy --release
cargo test -p volna-core       # headless viewer-core tests
volna/volna/check.sh           # viewer crates and VS Code adapter
python3 bench/run.py all --scale small   # quick benchmark (minutes)
python3 bench/run.py all                 # full suite (~1 h), updates results
```

The viewer crates are workspace members but excluded from `default-members`,
so core commands do not require GUI dependencies. Run either frontend with:

```sh
cargo run -p volna --profile viewer -- volna/volna/examples/picorv32.vtr
cargo run -p volna-egui --profile viewer -- trace.vtr
```

All verification is automated and headless; see [AGENTS.md](AGENTS.md) for the
CI and contribution rules.

## Quick start

```sh
# Convert existing traces
vtr convert sim.fst sim.vtr                 # lossless FST hierarchy + values
vtr convert sim.vcd sim.vtr                 # VCD
vtr convert pipeline.log pipeline.vtr       # Kanata/Konata (.log or .log.gz)
vtr convert trace.ftr trace.vtr             # FTR/LWTR4SC
vtr convert spans.json spans.vtr             # OpenTelemetry OTLP/JSON

# Export and cross-check
vtr2vcd trace.vtr trace.vcd                  # VTR -> VCD; also: vtr to-vcd
vtr fst-to-vcd trace.fst ref.vcd             # FST -> VCD via fst-reader
vtr vcd-compare ref.vcd trace.vcd            # compare all value changes

# Inspect
vtr info sim.vtr
vtr hier sim.vtr --vars --depth 3
vtr value sim.vtr top.cpu.pc 123450
vtr changes sim.vtr top.cpu.pc --from 100000 --to 200000
vtr dump sim.vtr --to 5000                   # VCD-like text
vtr log sim.vtr --severity warn              # log records rendered to text
vtr clocks sim.vtr                           # declared clocks: stretches and periods
vtr tx pipeline.vtr --stream cpu.thread0 --from 100 --to 200
vtr tx pipeline.vtr --id 42
```

Writing from Rust:

```rust
use vtr::*;
let mut w = Writer::create("out.vtr")?;
w.set_timescale(-9)?;
let top = Some(w.add_scope(None, "top", ScopeType::Module, "top")?);
let (_, clk) = w.add_var(top, "clk", VarType::Wire, Direction::Input,
                         SignalKind::Bits { width: 1, states: 4 })?;
let (_, data) = w.add_var(top, "data", VarType::Reg, Direction::Implicit,
                          SignalKind::Bits { width: 32, states: 4 })?;
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

Writing from C (the public header documents the complete API):

```c
vtr_writer *w = vtr_writer_create("out.vtr", NULL);
uint32_t top = vtr_writer_add_scope(w, VTR_NONE, "top", VTR_SCOPE_MODULE, NULL);
uint32_t node, clk;
vtr_writer_add_var(w, top, "clk", VTR_VAR_WIRE, VTR_DIR_INPUT,
                   VTR_SIGNAL_BITS, 1, 4, &node, &clk);
vtr_writer_set_time(w, 0); vtr_writer_emit_bit(w, clk, VTR_LOGIC_0);
vtr_writer_set_time(w, 5); vtr_writer_emit_bit(w, clk, VTR_LOGIC_1);
if (vtr_writer_close(w) != VTR_OK) fprintf(stderr, "%s\n", vtr_last_error());
```

## Benchmarks

`python3 bench/run.py all` builds GTKWave's `fstapi.c`, libfstwriter,
LWTR4SC's FTR writer, wellen/fst-reader, and the Verilator VTR backend. It
prepares synthetic and simulator-integrated workloads, including RSA-256 and
the openC910 SoC running CoreMark, traces each to FST and VTR, and regenerates
`docs/BENCHMARK_RESULTS.md`. See [the methodology](docs/BENCHMARKS.md).

## Repository layout

| Directory | Responsibility |
|---|---|
| [core/](core/README.md) | VTR library and C ABI; separate VDB library and RTL debugging |
| [tools/](tools/README.md) | Trace CLI, inspection/conversion, and headless Volna server |
| [volna/](volna/README.md) | Toolkit-independent core, GPUI frontend, and minimal egui frontend |
| [integrations/](integrations/README.md) | Verilator tools/tests, standalone slang exporter, and future integrations |
| `bench/` | Rust drivers, orchestration, C/C++ harnesses, workloads, and results |
| `demos/` | Logging examples and UI prototypes |
| `docs/` | Architecture, specifications, APIs, research, and benchmark reports |
| `ext/` | Pinned external projects and reference implementations |

The nine Rust packages share the root workspace and lockfile: `vtr`,
`vtr-capi`, `vtr-vdb`, `vtr-cli`, `volna-server`, `vtr-bench`, `volna-core`,
`volna`, and `volna-egui`. Use `cargo -p <package>` to select one. Standalone
Python exporters live in `integrations/slang`; the Verilator backend is in the
pinned `ext/verilator` submodule with tooling in `integrations/verilator`.
First-party code belongs under its owning component, while shared benchmarks
remain in `bench/`.

## Design references

Checked-in references under `ext/` include `libfstwriter` (FST writer and
format reference), LWTR4SC (SystemC FTR format/API), Konata (viewer and Kanata
format), and wavepeek (waveform-query consumer and reader-API reference).
Research also draws on GTKWave FST (`fstapi.h`/`fstapi.c`), public
Synopsys Verdi waveform/design-database concepts, the
[OpenTelemetry tracing specification](https://opentelemetry.io/docs/specs/otel/trace/api/),
the [Konata/Kanata format](https://github.com/shioyadan/Konata), IEEE 1800
SystemVerilog VCD, IEEE 1666 SystemC, Apache Arrow/Parquet, Perfetto, Chrome
Trace Event Format, and the Surfer/wellen readers.

## License

Project code is MIT OR Apache-2.0. Submodules and bundled third-party assets
retain their own licenses; see [Volna's asset notices](volna/volna/THIRD_PARTY.md).
