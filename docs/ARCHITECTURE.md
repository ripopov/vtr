# Hardware debugging environment architecture

The goal is a unified debugging environment for RTL and ESL models, including
SystemC and gem5. VTR stores runtime traces, VDB supplies separate design
metadata and domain semantics, and Volna is the primary UI. Simulator adapters
produce the data; consumers such as the planned wavepeek integration query it
to support interactive and AI-assisted debugging.

## Component boundaries

| Component | Code | Responsibility |
|---|---|---|
| VTR | `core/vtr`, `core/vtr-capi` | Trace format, reader/writer, runtime hierarchy, waveforms, transactions, relations and logs; Rust and C APIs |
| VTR tools | `tools/vtr-cli`, `bench/vtr-bench` | Trace inspection/conversion and Rust benchmark drivers |
| VDB | `core/vtr-vdb`, `integrations/slang` | Separate design metadata, source index, semantics, RTL netlists and temporal driver tracing; standalone pyslang export |
| Volna | `volna/volna-core`, `volna/volna`, `volna/volna-egui` | Toolkit-independent viewer logic, main GPUI UI and minimal egui adapter |
| Integrations | `integrations/` | Simulator/consumer build glue and end-to-end integration checks |
| External projects | `ext/` | Pinned simulator forks, consumers and format references |

VTR must contain no presentation rules or design source-level semantics.
Log-site provenance (`log.file`, `log.line`, `log.func`) is allowed in VTR;
design source mappings, specialized types and driver/load relationships belong
in VDB, attached to trace identities without modifying the trace. VDB is intended
to cover pipeline and transaction domains as well as RTL; its current library
and exporters implement the RTL companion. The existing `vtr-vdb` package name
does not make VDB part of the VTR file format.

Volna's core owns document state, view models, interaction, load scheduling
and toolkit-neutral drawing. Frontends host widgets and render that output.
The GPUI frontend is the main feature target; egui verifies toolkit independence
and retains its current minimal functionality. The detailed current design and
remote-file direction are in [Volna's architecture](../volna/volna/ARCHITECTURE.md).

The intended remote boundary puts raw trace queries beside the file and leaves
VDB profiles, presentation and user annotations on the client. Current local
sessions and whole-history loading do not yet implement that remote service.

## Integration status

| Area | Present in this repository | Direction |
|---|---|---|
| Verilator / RTL | Pinned backend with direct VTR tracing, VDB export/source indexing, smoke/logging/VDB checks | Preserve elaborated source semantics and explicit trace/design identity |
| SystemC / ESL | C ABI usable from C++/SystemC; Verilator SystemC trace wrapper; FTR conversion and LWTR4SC reference/benchmarks | Broader SystemC instrumentation and ESL design semantics |
| gem5 / pipelines | Kanata conversion, Konata reference, VDB application note and pipeline UX demo | Native gem5 integration with VTR/VDB; no gem5 backend is implemented here |
| wavepeek / AI agents | Pinned reference consumer used to study waveform query needs | VTR/VDB integration for AI-assisted debugging; no adapter is implemented here |
| Volna / UI | VTR and FST waveforms through a shared core; GPUI native/web/VS Code and minimal native egui | VDB attachment, source and transaction views, remote queries |
| Surfer | Pinned external VTR/VDB consumer and source-view integration | Existing integration remains usable; Volna is the official UI |

See [integrations/README.md](../integrations/README.md) for entry points and
[GOAL.md](../GOAL.md) for requirements.

## Workspace and supporting material

The eight Rust packages share one root Cargo workspace and lockfile.
`cargo test` exercises the five default packages;
`volna/volna/check.sh` checks the three viewer packages and the VS Code adapter.
Native viewer checks still require the platform SDK and desktop services.

`bench/` contains the shared orchestrator, C/C++ harnesses, simulator workloads
and results. `docs/` contains the normative specification, API references,
research and benchmark reports.
`demos/` contains examples and UI prototypes, not additional production viewers.

The Surfer submodule uses the VTR and VDB libraries and C headers directly
from their canonical paths under `core/`.
