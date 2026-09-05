# GOAL: Design and implement VTR

VTR (Vibe Trace Record) is a new open-source trace file format and reference library for hardware
simulation traces. Think of it as an open FSDB-class trace store: one file that
holds signal waveforms, transaction streams, the elaborated design hierarchy,
and runtime relations between transactions.

This document states the goal, requirements, constraints, references, and
deliverables. It deliberately does not prescribe the file layout, encoding,
compression, indexing, or API shape. Research the references, design the
format and APIs yourself, and justify your choices in the deliverables.

## 1. Scope: what VTR captures

VTR captures only what cannot be derived statically from source code or from
YAML/JSON/XML configuration. Concretely, three kinds of data:

1. **Runtime simulation trace.** Signal value changes and transaction streams
   with timestamped events and attributes.
2. **Elaboration-time design hierarchy.** Scopes, instances, signals, streams,
   and their attributes, as instantiated at simulation time. Hierarchy belongs
   in VTR because it is often only known at elaboration (for example, built
   from an XML platform description at startup), not from source alone.
3. **Runtime semantic links.** Parent/child and other relations between
   transactions, across streams and across hierarchy. Example: a CPU
   instruction transaction is the parent of a NoC transaction, which in turn
   is the parent of activity inside a slave device. FTR is the source of
   inspiration here.

## 2. Scope: what VTR does not capture (the VTR/VDB split)

Follow the Verdi model of separate waveform and design databases:

- **VTR (like FSDB)** is the trace dump: data plus hierarchy plus relations.
- **VDB (Vibe Data Base)** is a separate, application-specific layer that
  adds semantics and presentation on top of an unchanged VTR file: colouring of
  signals, transactions or pipeline stages; which attribute marks a squashed
  instruction; source file and line where a signal or module is defined;
  driver/load annotations; and similar.

VDB is CSS-like: the same VTR data can be rendered many different ways by
different VDBs. Many VDBs will exist for different domains and all reuse the
same VTR trace. Therefore:

- No VDB format is part of this deliverable.
- VTR must not embed presentation or source-level semantics, but it must carry
  enough stable identity and attribute data that a VDB can attach to it
  reliably (for example, name a signal, stream, transaction type, attribute,
  or relation kind and have that reference survive across runs of the same
  design).

Illustrative use cases the VTR + VDB pairing must enable:

- **Pipeline viewer.** A Konata-style trace is representable as FTR-like
  transactions with timestamped events and relations. A VDB then defines
  stage colouring, which attribute means "flushed", lane assignment, and so
  on, producing a fully featured Konata pipeline view from plain VTR data.
- **RTL debugger.** For FST-like waveform data, a VDB enables annotated trace
  drivers, reverse debugging (stepping backwards), and a waveform view.
- **Automated / LLM-driven querying** as done by `ext/wavepeek`.

## 3. Feature coverage requirements

VTR must be able to represent, without loss, every feature currently
supported by all of the following. Enumerate each feature of each source in
your design document and show how VTR covers it:

1. **FST** (GTKWave): all scope and variable types, value kinds (bit vectors
   with 4-state and 9-state, reals, strings, enums, integers), hierarchy
   attributes, timescale, time ranges, blackout/dump-off regions, aliases,
   writer-side and reader-side features, random access, partial reads.
2. **FTR** (LWTR4SC / SystemC transaction recording): streams, generators,
   transactions with begin/end times, typed attributes at begin/record/end,
   relations between transactions across streams, and everything else the
   format and its C++ API support.
3. **Konata** pipeline traces (Kanata format): instruction lifecycle,
   stages and lanes, retirement and flush, dependencies between instructions,
   labels and per-cycle details, and everything else the format supports.
4. **OpenTelemetry Tracing API**: traces, spans with start/end time, span
   kinds, attributes, events, links, status, parent/child and cross-trace
   relations, resources and instrumentation scopes.

## 4. Efficiency requirements

VTR must beat both FST and FTR in every meaningful scenario:

1. **Space.** VTR files must be smaller on disk than the equivalent FST or FTR
   file.
2. **Write speed.** Writing VTR must be faster than writing FST or FTR, so that
   VTR tracing slows a simulation down less than any other tracing does.
3. **Read and navigation speed.** Reading and navigating VTR must be faster
   than FST for applications such as `ext/wavepeek`: opening a file, browsing
   hierarchy, fetching the value of a signal at a time, scanning value changes
   of a signal over a window, finding events matching a condition.

"Meaningful scenario" means realistic workloads: small and large RTL designs,
long simulations with few active signals, short simulations with many active
signals, wide buses, transaction-heavy SystemC/TLM runs, and pipeline traces
of out-of-order cores. Synthetic microbenchmarks alone are not sufficient.

## 5. API requirements

1. Reference implementation in **Rust**.
2. A **C wrapper with a stable C ABI** over the Rust implementation, usable
   from C, C++, and SystemC without Rust toolchain knowledge.
3. The APIs must match or beat FSDB, FST, and FTR on clarity: small surface,
   obvious lifetimes and ownership, no hidden global state, clear error
   reporting, streaming writer, random-access reader, and no need to read the
   whole file to answer a local query.
4. The writer must be usable from a running simulator with minimal overhead
   (see section 4).

## 6. Deliverables

1. **Reference implementation** in Rust with a C ABI wrapper, with tests.
2. **VTR file format specification** as HTML or Markdown, complete enough
   that an independent implementation can be written from it.
3. **VTR API specifications** for both the Rust and C APIs, as HTML or
   Markdown, generated or hand-written.
4. **Application note** explaining how to design a VDB-style database that
   supplements VTR. It must include a worked design of a VDB that implements
   a fully featured Konata-like pipeline viewer on top of a VTR trace, and
   should also outline the RTL-debugger VDB case.
5. **Benchmark suite and results report** covering section 4 against FST and
   FTR: methodology, workloads, hardware, raw numbers, and analysis. The
   benchmarks must be reproducible from the repository.
6. **Design rationale** document recording the researched alternatives and
   why the chosen format and API decisions were made.

## 7. References

Local submodules under `ext/`:

- `ext/libfstwriter` — FST writer library and format reference (GTKWave).
- `ext/LWTR4SC` — Lightweight Transaction Recording for SystemC; FTR format
  and API reference.
- `ext/Konata` — Konata pipeline visualizer and Kanata trace format
  (see its `docs/`).
- `ext/wavepeek` — Rust CLI for querying waveform dumps; target consumer for
  read-performance requirements and a reference for reader API ergonomics.

External:

- GTKWave FST format and API (`fstapi.h`, `fstapi.c`) and its documentation.
- Synopsys Verdi waveform and design database concepts (public documentation only).
- OpenTelemetry Tracing specification:
  https://opentelemetry.io/docs/specs/otel/trace/api/
- Konata / Kanata trace format documentation:
  https://github.com/shioyadan/Konata
- IEEE 1800 SystemVerilog VCD and IEEE 1666 SystemC (for value types,
  scopes, and timescales).
- Alternative trace and columnar formats worth studying for encoding and
  indexing ideas, e.g. Apache Arrow/Parquet, Perfetto, Chrome Trace Event
  Format, and the Surfer / wellen waveform readers.

## 8. Ground rules for the agent

- Research the references first; do not reinvent what they already solve
  well, and document what you borrowed and what you rejected.
- Do not embed VDB or presentation data in VTR.
- Keep the format versioned and extensible from the first release.
- Measure before claiming any efficiency win; every claim in section 4 must
  be backed by the benchmark report.
- Prefer a clean, small API over feature breadth in the wrapper layers.
- All code, docs, and benchmarks live in this repository and build from a
  clean checkout.
