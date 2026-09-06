# RTL Vibe Data Base, netlists, and temporal driver tracing

The `vtr-vdb` companion reads source semantics exported by slang or the pinned
Verilator integration into a separate JSON VDB, and queries immutable runtime waveforms
through the existing Rust VTR reader. It does not add source data, sections,
encodings, or presentation fields to VTR.

## Build and use

Python 3.10+ and the pinned slang Python bindings are required for export.
The layout dependency uses Rust edition 2024; this companion requires Rust
1.88+ and is validated on Rust 1.88 and 1.96. The core VTR crate retains its existing toolchain policy.
The Rust CLI can read an exported VDB without Python or slang installed.

```sh
python3 -m venv .venv-vdb
.venv-vdb/bin/pip install -r tools/vdb/requirements.txt
git submodule update --init ext/elkrs
cargo build --release -p vtr-vdb
.venv-vdb/bin/python tools/vdb/export.py --top top -o design.vdb.json design.sv
# Also accepts repeated -I include-directory and -D NAME=value, and multiple files.
target/release/vtr-vdb check design.vdb.json simulation.vtr
target/release/vtr-vdb trace design.vdb.json simulation.vtr top.q --time 26 --depth 14
```

`--prefix TOP` explicitly maps `top.q` to `TOP.top.q`, for simulators that
add a wrapper scope. Paths supplied to `trace` always use VDB names. There
is no heuristic suffix matching. A separate packed range suffix such as
`a [7:0]` is removed when matching a VTR path. Array indices without a space
are retained. Alias names can map to the same VTR signal ID and share the
loaded history.

`check` diagnoses missing paths, conflicting paths, incompatible widths or
value kinds, and module-definition mismatches when VTR records component
names. Missing signals make `check` exit 2; `trace` allows partial recordings
and explains missing data where encountered. Incompatible or ambiguous
mappings prevent either command. Extra trace signals are allowed.

The export contains a SHA-256 design identity covering the source manifest,
elaboration options, and exported semantics. A producer can record that
identity using the existing generic string file attribute `design.vdb_id`.
An attribute mismatch rejects attachment. Without it, the CLI explicitly
reports **structural matching only**: identical names and widths cannot prove
that two RTL revisions implement the same logic. Assignment evaluation also
compares predicted values to recorded values and reports disagreement.
Neither structural checks nor agreeing samples alone prove source identity.
Paths outside the export working directory remain absolute; use the same
working directory and source arguments for reproducible identities.

## Native Verilator export

The pinned `ext/verilator` produces both files with `--trace-vtr`; no Python or
slang installation is needed for this path. Build it with
`integrations/verilator/build.sh` and use the existing `VerilatedVtrC` harness.
Verilation writes `<Mdir>/<prefix>.vdb.json`. The generated model embeds that
same document, so it does not depend on the build directory at runtime.
Opening `simulation.vtr` writes `simulation.vdb.json` beside it; other trace
filenames have `.vdb.json` appended. Failure to write the companion is fatal.

```sh
verilator --cc --exe --build --trace-vtr --top-module top top.sv main.cpp
# The harness opens simulation.vtr using VerilatedVtrC.
./obj_dir/Vtop
vtr-vdb check simulation.vdb.json simulation.vtr
vtr-vdb trace simulation.vdb.json simulation.vtr top.q --time 26 --depth 14
vtr-vdb netlist simulation.vdb.json simulation.vtr top --time 26 --output top.svg
```

Set `VTR_INCLUDE` and `VTR_LIBDIR` as described in
[the integration guide](../integrations/verilator/README.md). The runtime
companion needs no `--prefix`: it includes `trace_binding`, containing the
actual model wrapper and an explicit original-symbol-path to recorded-path map.
The mapping includes enabled declarations and aliases; extra trace entries are
allowed. Missing entries remain missing, with no fallback to guessed names.
An explicit conflicting `--prefix` is rejected. The build-directory VDB has
no recording binding and can still be attached using an explicit prefix.

Both files share `design_id` / `design.vdb_id`. A recording-bound VDB requires
that VTR identity, and a mismatch is rejected. The native identity is SHA-256
of the compact UTF-8 export before `design_id` and `trace_binding` are added,
with the producer's deterministic field order. It covers elaborated semantics,
source/include content hashes, producer version and invocation options. Slang
uses its own sorted-key JSON serialization; identities are producer-specific,
not a cross-frontend equivalence claim. Source paths resolve in the export
working directory; keep it available or use absolute source paths.

The native exporter runs after parameter and type resolution, before width
commitment removes implicit event controls and before optimization, inlining,
or scheduling. It preserves module/generate/instance-array hierarchy, specialized
widths, local parameters (including generated-loop constants), enum constants,
source locations, ports, connections, process bodies and static dependencies.
Compiler-generated truncations remain conversions. Verilator normalizes packed
select indices to zero-based offsets; VDB expressions preserve that normalization
with explicit ranges. Distributed instance-array connections are represented as
slices; partial output connections retain the existing unsupported diagnostic.

The same evaluator limits below apply to both producers. Unsupported source
constructs retain connectivity and diagnostics. This is an RTL connectivity
view, not a synthesized gate netlist. Verilator's two-state simulation determines
the recorded values; VDB does not restore unrecorded X/Z or event-region detail.
Trace depth/filtering may produce an incomplete recording even though VDB retains
the design. One recording currently accepts one elaborated model. The companion
contains source-level design information, including when tracing is combined with
`--protect-ids` (which already issues Verilator's `INSECURE` warning).

Native end-to-end verification:

```sh
cargo build --release -p vtr-capi
cargo build -p vtr-vdb
python3 integrations/verilator/vdb/run.py --verilator /path/to/verilator
```

This builds and simulates the existing four RTL fixtures plus an operator
fixture, checks real recordings with the CLI, and parses an SVG for every module.
It covers enables/holds, async and sync resets, pre-event pipeline dependencies,
source-order assignments, casts, signed arithmetic, bit indexing, hierarchy
references, enums, wide values, missing mappings, identity mismatch, and model
prefixes. Unlike the standalone fixture tests, these values come from Verilator.

## Surfer source navigation

The pinned Surfer integration opens a local `.vtr` together with its same-stem
`.vdb` companion automatically; `.vdb.json` is accepted when `.vdb` is absent.
The preferred companion must pass the existing VDB/VTR attachment checks.
Invalid companions are reported in the application log without preventing
waveform viewing. Relative source paths resolve from the companion directory.
Right-click a displayed signal and choose **Go to source** to open its RTL
at the declaration line. Explicit recorded-path mappings and aliases are used;
no suffix guessing is performed. The source index belongs to the loaded
waveform document, so replacing a recording also replaces its attachment.

Real paired Verilator examples, matching FST recordings, RTL and regeneration
instructions live in `ext/surfer/examples/verilator`. Surfer tests compare
all recorded changes and hierarchy metadata and render both formats against
shared image snapshots. The source-navigation snapshot clicks the actual
context-menu entry. See the [Surfer chapter](../ext/surfer/docs/html/source-code.html)
for ownership and limits. Driver tracing and netlist rendering remain in the
VDB library/CLI; this Surfer change exposes source navigation.

## Module netlists with recorded values

```sh
target/release/vtr-vdb netlist design.vdb.json simulation.vtr top \
  --time 26 --output top.svg
target/release/vtr-vdb netlist design.vdb.json simulation.vtr top.u0 \
  --time 26 --output stage.svg
```

The instance argument selects exactly one elaborated module. Its local signals,
operators, processes, and immediate child instances form the graph. Children are
opaque blue blocks with named ports; open the child's instance path to inspect
its internals. Generate scopes belong to their enclosing module. Parameterized
instances keep their elaborated widths and constants. A `NetlistIndex` indexes
ownership once; making a module view never traverses descendant internals.
Elaboration still exports the whole design, with per-instance specialization;
v2 does not deduplicate parameterized definitions or stream JSON on demand.

This is an RTL connectivity view. Single combinational assignments become
operator nodes (including mux, slice, concatenation, replication, and casts).
Statement blocks remain process nodes with all statically read signals, outputs,
clock/reset events, and source locations. It preserves procedural dependencies
without claiming a synthesized gate implementation. Unsupported statement or
expression evaluation retains statically known connectivity and a diagnostic.
Unconnected ports remain visible, and inout/ref pins use `↔`; the view does not
simulate electrical resolution. Complex output lvalues are explicitly marked
unsupported. Primitive gates/interfaces are outside the current exporter subset.

Signal nodes and instance/process pins show settled values read from VTR at the
requested timestamp. X/Z remain literal, buses wider than 64 bits are supported,
and long names/values have full text in SVG tooltips. `unavailable` appears in red
for missing recordings, out-of-range times, or invalidated samples across dump
gaps. Operator intermediate results are not invented. `--prefix TOP` has the
same exact attachment rules as driver tracing. Sampling is per visible symbol,
with histories shared by VTR signal ID; it currently loads full histories of
those signals. Layout can be reused while changing timestamps.

The native Rust [elkrs maintenance fork](https://github.com/ripopov/elkrs) is
pinned by the `ext/elkrs` gitlink and integrated as a Cargo path dependency.
Its layered algorithm places fixed-side ports and routes orthogonal wires.
SVG uses `data-instance` for child navigation by a host UI, plus stable view-local
`data-node`, `data-pin`, and `data-edge` identities. The standalone SVG is a
snapshot; CLI selection or a host UI provides drill-down. No layout or VDB data
is embedded in VTR.

### Netlist regression and visual review

```sh
cargo test -p vtr-vdb
.venv-vdb/bin/python tools/vdb/test_export.py
# Only when intentionally updating reviewed results:
VTR_UPDATE_GOLDENS=1 cargo test -p vtr-vdb --test netlist
# Requires librsvg's rsvg-convert; parses every SVG and renders review PNGs:
python3 tools/vdb/verify_svg.py
# Then open target/netlist-svg/review/index.html and inspect each diagram.
```

The suite has 30 netlist tests, 26 of which render annotated SVGs (including CLI
coverage), with 25 distinct checked-in SVG goldens in
`crates/vtr-vdb/tests/goldens`. Tests check known pipeline values, missing-data
behavior, immediate-child boundaries, feedback, port directions, generated and
nested instances, unsupported-process inputs, XML escaping, wide and four-state
values, layout determinism, and unchanged parent geometry with 2,000 added
descendants. Geometry checks verify block separation, orthogonal routes, and
exact wire-to-port attachment before golden comparison. Four committed VDB
fixtures are checked against live slang elaboration. Review notes are in
[NETLIST_REVIEW.md](NETLIST_REVIEW.md).

## Dependency tree and time model

Times are unsigned integers in the trace's timescale. A root query uses the
settled recorded value at that timestamp. A `25- (pre-event)` child means
the last recorded value strictly before timestamp 25. It is not a delta
cycle, and does not assert simulator event-region ordering.

For the test pipeline, a query of `top.q` at 26 follows these data edges
(control and clock children omitted here for readability):

```text
top.q = 00000011 @ 26
  top.u1.q = 00000011 @ 26
    top.u1.d = 00000011 @ 25- (pre-event)
      top.first = 00000011 @ 25- (pre-event)
        top.u0.q = 00000011 @ 25- (pre-event)
          ! hold at 15: no active assignment; searching previous event
          ! PosEdge top.u0.clk @ 5, event #1
          top.u0.d = 00000011 @ 5- (pre-event)
```

The actual stdout includes source file/line/column on each node, assignment
locations, recorded values, event times and per-event-signal occurrence
numbers, and the reason for each dependency (data, mux selection, if control,
bit-select index, or event trigger). Event numbers count definite matching
edges since the start of the recorded history; they are not an assumed global
cycle count shared by different clocks.

Combinational execution follows source order. Blocking temporary values
substitute their dependencies, and later assignments overwrite earlier ones.
False conditional overwrites preserve the condition that kept an earlier
assignment active. Muxes follow the selected branch; guards and short-circuit
controls appear separately from data dependencies.

For sequential drivers, the engine searches clock/reset transitions backward,
including events at which the register did not change. It evaluates the
process at each event. If an enable holds the register, it retains the control
dependencies and searches earlier events. Nonblocking RHS values use the
pre-event trace; event signals themselves use post-transition values so an
asynchronous reset assertion selects the reset branch. Recursion into an
upstream register excludes the downstream sampling edge and finds the prior
relevant event. Synchronous resets are ordinary pre-edge control inputs.

Depth counts signal dependency edges, including hierarchy connection edges,
controls, and clocks. Zero prints only the selected signal. The maximum is
128; a separate 10,000-node budget and a recursion-cycle guard prevent runaway
trees. Truncation is printed explicitly. CLI syntax, file, and attachment
errors exit 2. A completed tree query exits 0 even if branches contain explicit
ambiguity, unsupported-semantics, or missing-data diagnostics.

## VDB v2 schema

The UTF-8 JSON object is identified by `format: "vtr-rtl-vdb"`, `version: 2`.
Version 1 must be re-exported from RTL: it lacks ownership and complete ports.
Unknown format versions are rejected. Unknown object fields are ignored for
additive metadata evolution; unknown expression/statement tags are rejected.
This is an application format independent of the VTR container version.
`crates/vtr-vdb/tests/fixtures/*.vdb.json` are complete, reproducible examples.

| Field | Meaning |
|---|---|
| `producer`, `top`, `options` | frontend version, selected root module, elaboration arguments |
| `sources` | source/include file paths and SHA-256 content hashes |
| `design_id` | SHA-256 identity of source/options/semantics; see producer serialization rules above |
| `trace_binding` (optional) | recording wrapper `prefix` and `signals` map from original symbol paths to exact recorded paths; requires matching VTR identity |
| `instances` | elaborated instance `path`, `parent` module instance (null for root), definition name, source location, ordered `ports` (`name`, `symbol`, `direction`) including unconnected and top-level ports |
| `symbols` | map keyed by elaborated hierarchical path; each value repeats `path`, and contains module `owner`, `kind`, `type`, `source`, and optional constant `value` |
| `connections` | owning child `instance`, child port path, slang direction (`In`, `Out`, etc.), connection expression, source |
| `processes` | numeric local ID, module `owner`, `origin` (`rtl` or synthetic `connection`), static `reads`, mode (`comb`, `seq`, `unsupported`), target paths, event controls, body, source |

Every source location contains `file`, one-based `line`, and `column`.
Types record slang's resolved text, integral bit `width`, `signed`,
`four_state`, and `integral` flags. Parameter and enum constants retain slang's
value spelling. Values in expressions use their elaborated, context-sized
types, including implicit conversions. Symbol IDs are paths, never VTR IDs.
Process IDs are local to an export and carry no cross-run identity.

Each expression has `kind`, `type`, and `source`, with these payloads:

| Kind | Payload |
|---|---|
| `NamedValue`, `HierarchicalValue` | `symbol` path |
| `Constant` | `value` in slang integer spelling |
| `Conversion` | `operand` |
| `UnaryOp` | slang operator name `op`, `operand` |
| `BinaryOp` | `op`, `left`, `right` |
| `ConditionalOp` | `cond`, `yes`, `no` |
| `Concatenation` | ordered `operands` |
| `ElementSelect` | `value`, `selector`, declared `range_left`, `range_right` |
| `Replication` | constant `count`, `operand` (kept compact, not expanded) |
| `RangeSelect` | `value`, `left`, `right`, slang `selection`, declared `range_left`, `range_right` |
| `Unsupported` | `reason`, static symbol `reads` |

Statements are `Empty`; `Sequence {statements}`;
`Assign {target, value, nba, source}`;
`If {cond, yes, no, source}`; or `Unsupported {reason, source}`.
Sequential events contain `edge` (`PosEdge`/`NegEdge`), `expr`, and `source`.
Unsupported statement bodies still retain their statically discovered reads and targets,
so they cannot silently become supported assignments. Slang errors abort export.

## Supported semantics and limits

The supported evaluation subset is synthesizable scalar/packed integral RTL
with values 1–64 bits wide: continuous assignments, `always_comb`, `always @*`,
sequential statement blocks, if/else, blocking combinational temporaries,
whole-signal assignments, direct input/output port connections, and
nonblocking edge-triggered `always` / `always_ff` processes. Multiple simple
posedge/negedge controls support asynchronous resets and multiple clocks.
Hierarchy and parameter elaboration are performed by slang, not by parsing
source text in the debugger.

Supported expression operations include arithmetic `+ - * / %`, bitwise
operators, logical short-circuit operators, comparisons, shifts, integer casts,
ternaries, concatenation, replication, and packed bit selection. Unary reductions AND, OR,
and XOR are supported. X/Z data propagates an unknown numerical result; X/Z
control produces an explicit ambiguous branch instead of claiming a definite
provenance. This is conservative provenance analysis, not a four-state simulator.

Unsupported operations produce diagnostics: cases and loops, latches and
incomplete combinational assignment, partial/aggregate lvalues, function/task
execution, interfaces/inout/ref resolution, strength resolution, force/release,
procedural delays, event `iff`, complex clock expressions, blocking sequential
assignments, nonblocking combinational assignments, initialization processes,
and evaluation of range selections, aggregates, real/string values or widths over 64 bits.
Declarations and type text can still be exported for unsupported data types.
Only the documented procedural/continuous and simple port drivers are analyzed;
primitive gates and other simulation mechanisms are not modeled. A leaf says
“external input / no elaborated driver”, rather than proving it is an input.

VTR timestamps do not identify active/NBA regions. The engine assumes ordinary
settled synchronous waveforms with external inputs stable across sampling edges.
It flags observed external-input changes at sampled edges, repeated clock
transitions at one timestamp, unknown clock transitions, and assignment/value
mismatches. Combinational glitches and simultaneous asynchronous stimulus can
be ambiguous even if final values happen to agree. The tool cannot reconstruct
unrecorded delta-cycle causality or certify the absence of races.

Missing mappings, missing initial samples, out-of-range queries, dumping-off
intervals, stale samples following dump resumption, and unavailable prior
assignment events are explicit. An initial value suppressed by trace writer
deduplication may be indistinguishable from a missing initial sample. Event
search across a recorded dump gap is refused. Full signal histories are loaded
lazily and cached per VTR ID for the query; this first implementation favors
clear semantics over bounded-window scanning. No performance improvement is
claimed and no VTR decode path is changed.

## Validation

```sh
# Rust tests use committed slang-generated fixtures; no Python dependency.
cargo test -p vtr-vdb
# Re-elaborate pipeline fixtures while running temporal and stdout assertions.
VTR_VDB_PYTHON="$PWD/.venv-vdb/bin/python" cargo test -p vtr-vdb
# Export errors, parameters, hierarchy, driver retention, fixture reproducibility.
.venv-vdb/bin/python tools/vdb/test_export.py
cargo clippy -p vtr-vdb --all-targets -- -D warnings
cargo test
```

The RTL fixtures cover muxes, arithmetic, source-order combinational assignment,
registers, a two-stage parameterized hierarchical pipeline, enables, synchronous
and asynchronous resets, last-write NBA behavior, and depth limits. Matching VTR
snapshots are explicitly constructed with the reference writer; assertions check
both dependency identities/times and actual CLI stdout. Negative tests exercise
mapping, missing history, X/Z controls, multiple drivers, dump gaps, and design
identity mismatches. The fixtures do not claim independent simulator parity.
