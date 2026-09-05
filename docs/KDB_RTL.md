# RTL knowledge database and temporal driver tracing

The `vtr-kdb` companion uses slang to elaborate SystemVerilog, exports source
semantics into a separate JSON KDB, and queries immutable runtime waveforms
through the existing Rust VTR reader. It does not add source data, sections,
encodings, or presentation fields to VTR.

## Build and use

Python 3.10+ and the pinned slang Python bindings are required for export.
The Rust CLI can read an exported KDB without Python or slang installed.

```sh
python3 -m venv .venv-kdb
.venv-kdb/bin/pip install -r tools/kdb/requirements.txt
cargo build --release -p vtr-kdb
.venv-kdb/bin/python tools/kdb/export.py --top top -o design.kdb.json design.sv
# Also accepts repeated -I include-directory and -D NAME=value, and multiple files.
target/release/vtr-kdb check design.kdb.json simulation.vtr
target/release/vtr-kdb trace design.kdb.json simulation.vtr top.q --time 26 --depth 14
```

`--prefix TOP` explicitly maps `top.q` to `TOP.top.q`, for simulators that
add a wrapper scope. Paths supplied to `trace` always use KDB names. There
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
identity using the existing generic string file attribute `design.kdb_id`.
An attribute mismatch rejects attachment. Without it, the CLI explicitly
reports **structural matching only**: identical names and widths cannot prove
that two RTL revisions implement the same logic. Assignment evaluation also
compares predicted values to recorded values and reports disagreement.
Neither structural checks nor agreeing samples alone prove source identity.
Paths outside the export working directory remain absolute; use the same
working directory and source arguments for reproducible identities.

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

## KDB v1 schema

The UTF-8 JSON object is identified by `format: "vtr-rtl-kdb"`, `version: 1`.
Unknown format versions are rejected. Unknown object fields are ignored for
additive metadata evolution; unknown expression/statement tags are rejected.
This is an application format independent of the VTR container version.
`crates/vtr-kdb/tests/fixtures/*.kdb.json` are complete, reproducible examples.

| Field | Meaning |
|---|---|
| `producer`, `top`, `options` | pinned slang version, selected root module, elaboration arguments |
| `sources` | source/include file paths and SHA-256 content hashes |
| `design_id` | SHA-256 of the canonical sorted-key JSON document before this field is added |
| `instances` | elaborated instance path, definition name, source location |
| `symbols` | map keyed by elaborated hierarchical path; each value repeats `path`, and contains `kind`, `type`, `source`, and optional constant `value` |
| `connections` | child port path, slang direction (`In`, `Out`, etc.), connection expression, source |
| `processes` | numeric local ID, mode (`comb`, `seq`, `unsupported`), target paths, event controls, body, source |

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
| `Unsupported` | `reason` |

Statements are `Empty`; `Sequence {statements}`;
`Assign {target, value, nba, source}`;
`If {cond, yes, no, source}`; or `Unsupported {reason, source}`.
Sequential events contain `edge` (`PosEdge`/`NegEdge`), `expr`, and `source`.
Unsupported statement bodies still retain their statically discovered targets,
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
ternaries, concatenation, and packed bit selection. Unary reductions AND, OR,
and XOR are supported. X/Z data propagates an unknown numerical result; X/Z
control produces an explicit ambiguous branch instead of claiming a definite
provenance. This is conservative provenance analysis, not a four-state simulator.

Unsupported operations produce diagnostics: cases and loops, latches and
incomplete combinational assignment, partial/aggregate lvalues, function/task
execution, interfaces/inout/ref resolution, strength resolution, force/release,
procedural delays, event `iff`, complex clock expressions, blocking sequential
assignments, nonblocking combinational assignments, initialization processes,
and evaluation of aggregates, real/string values or widths over 64 bits.
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
cargo test -p vtr-kdb
# Re-elaborate pipeline fixtures while running temporal and stdout assertions.
VTR_KDB_PYTHON="$PWD/.venv-kdb/bin/python" cargo test -p vtr-kdb
# Export errors, parameters, hierarchy, driver retention, fixture reproducibility.
.venv-kdb/bin/python tools/kdb/test_export.py
cargo clippy -p vtr-kdb --all-targets -- -D warnings
cargo test
```

The RTL fixtures cover muxes, arithmetic, source-order combinational assignment,
registers, a two-stage parameterized hierarchical pipeline, enables, synchronous
and asynchronous resets, last-write NBA behavior, and depth limits. Matching VTR
snapshots are explicitly constructed with the reference writer; assertions check
both dependency identities/times and actual CLI stdout. Negative tests exercise
mapping, missing history, X/Z controls, multiple drivers, dump gaps, and design
identity mismatches. The fixtures do not claim independent simulator parity.
