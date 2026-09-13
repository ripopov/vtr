# AGENTS.md

Instructions for coding agents working in this repository. The project's
purpose, scope, requirements and ground rules are in [GOAL.md](GOAL.md);
read it first and treat its section 8 as binding.

## What this is

A unified hardware debugging environment for RTL and ESL models, built around
VTR runtime traces, separate VDB design metadata, and the Volna UI. SystemC,
gem5 and wavepeek integration are part of the direction; see
`docs/ARCHITECTURE.md` for current support and planned work.

One Rust workspace with nine crates grouped by responsibility:
`core/` contains `vtr`, `vtr-capi`, `vtr-vdb` and `vtr-query`; `tools/` contains `vtr-cli`;
`bench/` contains `vtr-bench`; `volna/` contains `volna-core`, `volna` (GPUI)
and `volna-egui`. Standalone Python exporters live in `integrations/slang`.
The Verilator backend is in the pinned `ext/verilator` submodule, with build
tools in `integrations/verilator`.
Shared benchmarks remain in `bench/`. Add first-party code under its owning
component.

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

## Volna direction

Volna is the official viewer and is expected to outgrow its current shape
(one GPUI wave view over an in-memory or memory-mapped file). Three intents
guide its evolution; they describe direction, not a fixed object tree, and
`volna/volna/ARCHITECTURE.md` records the current state.

1. **Toolkit-independent core.** Everything that decides what is shown and
   what an input does (document state, view models, viewport math, cursor
   and selection, load scheduling, painting into a toolkit-neutral display
   list) belongs in a core that imports no GUI toolkit and is testable
   headless on every platform. Frontends paint and host native widgets;
   they do not hold viewer logic. `volna/volna-core` is that core; keep
   new viewer behaviour there, not in a frontend.
2. **Several frontends over one core.** `volna` (GPUI, native and wasm) is
   the main viewer and the target for new features. `volna-egui` exists to
   prove that `volna-core` is toolkit-independent, not to provide a second
   feature-complete viewer. Do not add new features to `volna-egui` or
   require feature parity with `volna`. As the core and main viewer evolve,
   keep `volna-egui` building and working with its current minimal feature
   set, making only the compatibility fixes and maintenance needed to
   preserve that functionality. Other frontends should be
   thin adapters of the same core, not forks of the viewer. Shared code is
   the dense data canvases (waves, tables, pipeline timelines); the chrome
   (trees, lists, menus, dialogs) uses each toolkit's own widgets. Saved
   viewer state (open trace, tabs, displayed signals, cursor, markers,
   layout) is core data in a frontend-neutral format, so a session saved
   in one frontend can be read by another without requiring that frontend
   to implement every view or feature in the session.
3. **Client-server split for remote files.** The main use case is VS Code
   in remote mode (`vscode-server` over SSH, tunnels, containers) opening
   very large VTR files that live on the remote host, without transferring
   the file to the client. The reader and all query work (hierarchy,
   histories, level-of-detail summaries, row and transaction windows,
   search) run next to the file as a query library; the viewer talks to it
   through a session interface that has a local in-process implementation
   and a remote one over a compact binary protocol. Transfer must scale
   with what is on screen, not with file size, so the viewer must never
   block a frame on the network. The same split serves any webview-hosted
   frontend, where the sandbox cannot memory-map files.

Constraints that follow: the query library and the protocol carry raw
trace data only; presentation rules, VDB profiles and user annotations stay
on the client (GOAL.md section 2 applies to the wire as much as to the
file). The native app keeps working over a memory-mapped file through the
local session, with no second code path.

## Long-term agent-driven debugging

Support MCP (Model Context Protocol) so an AI agent can drive an investigation
from its existing conversation, query traces, and launch or control Volna to
show hypotheses and evidence. Users should be able to select signals or time
regions and add comments in Volna to steer that same investigation. An initial
feedback path can compose a message to copy into the agent conversation,
including a stable selection snapshot and readable trace, signal and time
references. Keep MCP adapters thin over toolkit-independent core commands and
query APIs, preserving the VTR/VDB split. This is a long-term direction, not
a claim of implemented support or a requirement for embedded agent chat.

## Where to look

| need | file |
|---|---|
| repository boundaries, layout and integration status | `docs/ARCHITECTURE.md`, `integrations/README.md` |
| file format (normative) | `docs/SPEC.md` |
| why things are the way they are, what was tried and rejected | `docs/RATIONALE.md` |
| APIs | `docs/API_RUST.md`, `docs/API_C.md` |
| Volna viewer: toolkit-free core, GPUI frontend (native/web/VS Code), egui frontend (native) | `volna/volna/ARCHITECTURE.md`, `volna/volna/README.md`, `volna/volna-egui/README.md`, `volna/volna/VERIFICATION.md` |
| logging (log sites, `LOG_BLOCK`, C++ header, comparison with NanoLog/binlog/Quill/CLP) | `docs/LOGGING.md`, `bench/log/` |
| benchmark method / current numbers | `docs/BENCHMARKS.md`, `docs/BENCHMARK_RESULTS.md` |
| RTL VDB schema, Verilator/pyslang exporters, Surfer attachment | `docs/VDB_RTL.md` |
| VDB source index (`verilator_vdb_index`, slang submodule of the Verilator fork) and Surfer's source tile | `integrations/verilator/README.md`, `docs/VDB_RTL.md`, `ext/surfer/docs/html/source-code.html` |
| open ideas ranked by measured headroom | `docs/SOTA_REVIEW_2026.md` |
| Volna panels, docking and persistent workspace sessions (design proposal) | `docs/workspaces.html` |

## Build and test

```sh
cargo build --release          # library, libvtr.{a,so}, vtr CLI, vtr-bench
cargo test                     # unit, round-trip, converter and C-ABI tests
cargo clippy --release
volna/volna/check.sh           # viewer crates (core, GPUI, egui); uses pinned nightly and platform SDK
python3 bench/run.py all --scale small   # quick benchmark (minutes)
python3 bench/run.py all                 # full suite (~1 h), regenerates docs/BENCHMARK_RESULTS.md
```

The viewer crates (`volna-core`, `volna`, `volna-egui`) are explicit workspace
members, excluded from `default-members` to keep the core build independent of
GUI dependencies. Run a frontend with `cargo run -p volna --profile viewer --
trace.vtr` or `cargo run -p volna-egui --profile viewer -- trace.vtr`;
`cargo test -p volna-core` runs the headless viewer tests on any platform. The
viewer currently displays VTR and FST waveforms; VDB integration is planned and must
preserve the VTR/VDB split. Changes to the viewer should move it toward, not
away from, the intents in "Volna direction" above.

## Rules

- **Research first; prioritize clean formats and APIs.** We are currently
  in the research phase, so backward compatibility of the Rust API, C API/ABI,
  and file format is not a requirement. Prioritize clean, coherent formats
  and APIs over preserving existing interfaces or behavior. Favor breaking
  changes over compatibility workarounds when they simplify the design.
  Refactor directly rather than adding unnecessary indirection. Aim for the simplest,
  clearest solution, and update affected implementations, callers, tests,
  and documentation together. Version formats to reject incompatible files
  clearly; support for older versions is not required (GOAL.md section 8).
- **Fix missing abstractions at their owning layer.** Before adding a cache,
  side table, flag, or adapter workaround, check whether it duplicates
  information already owned elsewhere. If a consumer needs a missing query
  over that information, prefer adding a small, coherent API at the owning
  layer and updating callers. Keep derived caches only when they serve a
  demonstrated performance need, with clear ownership and consistency rules.
- **Reassess design when scope changes.** When a task expands across
  components, revisit earlier implementation choices. Do not preserve a local
  workaround merely because it was already implemented or committed.
  Minimizing the diff is secondary to achieving the simplest coherent design
  across the affected components.
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
  a round-trip test in `core/vtr/tests/`, and a rationale entry. Breaking
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
- Keep documentation focused on current architecture, behavior, requirements
  and reproducible workflows. Use Git for history; do not append dated progress
  reports, migration diaries or per-change verification records.
- Docs are part of the deliverable: update `SPEC.md`, `RATIONALE.md` and
  the API references in the same change as the code.
- Commit messages describe what changed and the measured effect; no
  attribution trailers.
