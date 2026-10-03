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
| Volna trace loading | `volna/volna-trace` | Standalone recording access, immutable raw data, local/remote sessions, memory admission and complete-object client/protocol |
| Volna | `volna/volna-core`, `volna/volna`, `volna/volna-egui` | Toolkit-independent viewer logic, main GPUI UI and minimal egui adapter |
| Volna server | `tools/volna-server` | Headless complete-object loading over framed stdin/stdout; uses `volna-trace`, independently of the viewer core |
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
and toolkit-neutral drawing. Its panels and versioned workspace codec own
dock layout, linked navigation and persistent session state. GPUI native and
VS Code hosts save separate JSON sidecars; VTR and VDB remain unchanged.
Frontends host widgets and render that output. Recording access is below the
viewer, in `volna-trace`:

```text
volna / volna-egui → volna-core → volna-trace → VTR / FST readers
                                      ↑
                                volna-server
```

Neither the trace library nor the server depends on the viewer core, a frontend
or a GUI toolkit. The library does not depend on the server. Local viewing
opens files in process; remote viewing drives the library's client over the
host transport to the server beside the file.
The GPUI frontend is the main feature target; egui verifies toolkit independence
and retains its current minimal functionality. The detailed current design and
remote-file direction are in [Volna's architecture](../volna/volna/ARCHITECTURE.md).

The remote boundary loads complete selected histories and transaction tracks
beside the file, with navigation and analysis over resident client data.
VDB profiles, presentation and user annotations stay on the client.
`volna-server` implements the headless loading service. The VS Code frontend
opens workspace traces through its child relay and loads selected full histories
and transaction tracks asynchronously. Native execution and the remote client
use the same immutable raw objects from `volna-trace`; the viewer core adapts
its document-tagged requests to raw loads. The server calls the same session
backend as local execution. Viewer analysis and preparation stay in the core.
All transaction navigation uses complete resident objects. Transport framing,
backpressure and remote admission limits stay in the protocol adapter. See
[verification](../volna/volna/VERIFICATION.md) for measured coverage and limits.

## Integration status

| Area | Present in this repository | Direction |
|---|---|---|
| Verilator / RTL | Pinned backend with direct VTR tracing, VDB export/source indexing, smoke/logging/VDB checks; the `vtr_trace` package (declared clocks, pipeline trackers) with a standalone sink for other simulators | Preserve elaborated source semantics and explicit trace/design identity |
| SystemC / ESL | C ABI usable from C++/SystemC; Verilator SystemC trace wrapper; FTR conversion and LWTR4SC reference/benchmarks | Broader SystemC instrumentation and ESL design semantics |
| gem5 / pipelines | Kanata conversion, Konata reference, VDB application note and pipeline UX demo; SystemVerilog pipeline tracers over `vtr_track.hpp`, proven on the openC910 with a differential checker | Native gem5 integration with VTR/VDB over the same keyed tracker; no gem5 backend is implemented here |
| wavepeek / AI agents | Pinned reference consumer used to study waveform query needs | VTR/VDB integration for AI-assisted debugging; no adapter is implemented here |
| Volna / UI | VTR and FST waveforms through a shared core; GPUI native/web/VS Code and minimal native egui | VDB attachment, source and transaction views, complete-object remote loading |
| Surfer | Pinned external VTR/VDB consumer and source-view integration | Existing integration remains usable; Volna is the official UI |

See [integrations/README.md](../integrations/README.md) for entry points and
the [project README](../README.md#project-requirements) for requirements.

## Workspace and supporting material

The eleven Rust packages share one root Cargo workspace and lockfile.
`cargo test` exercises the six default packages.
`python3 tools/check-volna-loading.py` builds and tests the trace library and
server independently and rejects viewer/toolkit dependencies, including
optional-feature, development and platform-specific edges.
`volna/volna/check.sh` also checks the three viewer packages and VS Code adapter.
Native viewer builds require the platform SDK; verification is automated and
headless.

`bench/` contains the shared orchestrator, C/C++ harnesses, simulator workloads
and results. `docs/` contains the normative specification, research and
benchmark reports. API documentation lives with the code: rustdoc in each
crate and the C header `core/vtr-capi/include/vtr.h`.
`demos/` contains examples and UI prototypes, not additional production viewers.

The Surfer submodule uses the VTR and VDB libraries and C headers directly
from their canonical paths under `core/`.
