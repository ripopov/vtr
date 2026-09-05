# Application note: building a VDB on top of VTR

VTR stores what happened in a simulation. It deliberately stores nothing
about how to *show* it. A VDB (Vibe Data Base) is the separate,
application-specific layer that adds meaning and presentation to an
unchanged VTR file: which attribute marks a squashed instruction, which
stage is "execute", what colour a lane gets, where in the source a module
is defined. The same VTR file can be rendered by many VDBs, and a VDB
written for one design keeps working across simulation runs because it
attaches to VTR by *names*, never by ids.

This note explains the attachment contract, gives a complete worked VDB
design for a Konata-style pipeline viewer, and outlines the RTL-debugger
VDB.

## 1. The attachment contract

### 1.1 Stable identity in VTR

| VTR object | stable key | unstable (per file) |
|---|---|---|
| scope, var, stream, generator | full hierarchical path of names (`top.cpu0.pipe`), plus the node kind | node id, signal id |
| signal | the path of any var that declares or aliases it | signal id |
| transaction type | generator path (`cpu.thread0.instruction`) | generator node id |
| transaction | its generator plus its position in time; optional producer ids stored as attributes (`insn_id_in_sim`, `otel.span_id`, `ftr.id`) | transaction id |
| attribute | key string | — |
| event, stage, lane | name string | — |
| relation | kind string | — |
| scope/var meaning | scope type and var type codes (FST numbering) | — |

Producers should choose names that are stable across runs (they usually
are: HDL hierarchy, generator names, attribute keys). Numeric ids are
assigned per file and a VDB must resolve names to ids at load time
(`Reader::find_node`, `find_signal`, or a walk over `Hierarchy::nodes`).

### 1.2 What a VDB is

A VDB is a document (JSON, YAML, SQLite, a Rust struct — VTR does not care)
whose entries are *selectors* plus *semantics*:

* a selector names VTR objects by path/kind/key pattern (glob or regex on
  the full path; an attribute key; a stage or lane name; a relation kind);
* the semantics say what those objects mean to the application (roles,
  colours, groupings, source locations, formatting).

Loading a VDB means evaluating every selector against the hierarchy and
the string table once, producing id-keyed tables the viewer uses. Nothing
is written back to the VTR file.

### 1.3 Reading pattern

A VDB-driven viewer opens the file (cheap: directory, strings, hierarchy),
resolves selectors, and then issues local queries as the user navigates:
`value_at`, `changes` in a window, `visit_transactions` with a window and
generator filter, `relations_from/to`. None of these read the whole file
(see `docs/SPEC.md` section 6.5 and 7.3).

## 2. Worked design: a Konata-style pipeline viewer

### 2.1 What is in the VTR file

`vtr convert trace.log trace.vtr` (or a simulator writing VTR directly
through `vtr_writer_*`) produces:

* one scope `cpu` (type `core`), one stream per hardware thread named
  `thread<N>` (kind `PIPELINE`), one generator `instruction` per stream;
* one transaction per instruction: `begin` = fetch cycle, `end` = retire
  cycle, `status` = `unset` (retired) / `aborted` (flushed) / `open` (never
  retired);
* attributes: `insn_id_in_sim` (begin), `line` (begin), `label` and
  `detail` (record, text with real newlines), `retire_id` (end);
* stages: one per Kanata `S` line: `name` (e.g. `F`, `Dc`, `X`), `lane`
  (`0`, `1`, ...), `[begin, end]`; a stage started while another stage on
  the same lane is open closes that one (Kanata semantics); type-2 labels
  become stage attribute `label`;
* relations: `wakeup` from producer to consumer, attribute `type` when not
  the default;
* file attributes: `time.unit = "cycle"`, `kanata.version`,
  `kanata.start_cycle`.

The file therefore has every fact the Konata viewer draws, and nothing
about colours, rows, or which stage names mean what.

### 2.2 The VDB document

```yaml
vdb: 1
name: konata-riscv-ooo
applies_to:
  file_attr: { time.unit: cycle }          # sanity check on load
  streams: "cpu.thread*"                   # glob on stream path
  generator: instruction

rows:                                      # Y axis
  order: begin_time                        # or attribute: insn_id_in_sim
  key: { attribute: insn_id_in_sim }       # printed as s<gid>
  hide_when: { status: aborted }           # "hide flushed ops" toggle uses status

lanes:                                     # X-axis tracks per instruction
  "0": { role: pipeline, order: 0 }
  "1": { role: stall, order: 1, color: { h: 0, s: 0, l: 70 } }

stages:                                    # colour + role by stage name
  F:   { role: fetch,     color: { h: 250, s: 60, l: 60 }, long: Fetch }
  Dc:  { role: decode,    color: { h: 220, s: 60, l: 60 } }
  Rn:  { role: rename,    color: { h: 190, s: 60, l: 60 } }
  Ds:  { role: dispatch,  color: { h: 160, s: 60, l: 60 } }
  Is:  { role: issue,     color: { h: 130, s: 60, l: 60 } }
  X:   { role: execute,   color: { h: 100, s: 60, l: 60 } }   # dependency arrow anchor
  Cm:  { role: complete,  color: { h: 70,  s: 60, l: 60 } }
  Rt:  { role: retire,    color: { h: 40,  s: 60, l: 60 } }
  stl: { role: stall,     color: { h: 0,   s: 0,  l: 70 } }
  "*": { color: auto }                     # unknown stage names: derived hue

flush:
  status: aborted                          # what "squashed" means
  overlay: { color: { h: 0, s: 80, l: 50, a: 0.3 } }

dependencies:
  relation: wakeup
  from_anchor: { stage_role: execute, edge: end }     # producer side
  to_anchor:   { stage_role: execute, edge: begin }   # consumer side
  style: { by_attribute: type, default: solid, 1: dashed }

labels:
  name:   { attribute: label }             # left pane text
  detail: { attribute: detail }            # tooltip
  stage:  { stage_attribute: label }       # per-stage tooltip lines

stats:
  branch: { label_regex: "\\b(beq|bne|jal|jalr|b[a-z]+)\\b" }
  store:  { label_regex: "\\bs[bhwd]\\b" }
```

Every selector is a name: stream path glob, generator name, attribute
keys, stage and lane names, relation kind, status codes. The document is
independent of ids, of the number of instructions, and of the run.

### 2.3 Loading the VDB

1. Open the VTR file; check `file_attr` selectors against
   `Reader::meta().attrs`.
2. Resolve streams: walk `Reader::streams()`, keep those whose
   `full_path` matches the glob; for each keep the generator node named
   `instruction` (children of the stream).
3. Intern the attribute keys, stage names, lane names and relation kinds
   the VDB uses into string ids (`StringTable::find`, or build a
   `HashMap<&str, StrId>` over the table once). Missing names are not
   errors: a VDB may describe more stages than a particular run used.
4. Build id-keyed tables: `stage_style[StrId]`, `lane_order[StrId]`,
   `role_of_stage[StrId]`, `label_key: StrId`, and so on.

### 2.4 Rendering with local queries

*Row set for the visible window* `[t0, t1]` (cycles):

```rust
let q = TxQuery { generator: Some(gen), window: Some((t0, t1)), ..Default::default() };
reader.visit_transactions(&q, |tx| { rows.push(row_from(tx, &tables)); true })?;
```

Only transaction blocks overlapping the window are decoded. For each
transaction the viewer has begin/end, status, attributes, and the stage
list; drawing a row is: for each stage look up `stage_style[name]` and
`lane_order[lane]`, draw `[begin, end)` on that lane (an open stage ends
at `tx.end`), overlay the flush colour when `status == aborted`.

*Dependency arrows*: `reader.relations_to(tx.id)` gives the producers of
a consumer (relations are indexed by target id per block). Anchor points
come from the stage with `role_of_stage == execute` on each side, exactly
as the VDB says; if a side has no such stage the arrow is not drawn.

*Labels and tooltips*: `label`/`detail` attributes and per-stage `label`
attributes, all by key id.

*Row identity across runs / A-B comparison*: the VDB names
`insn_id_in_sim` as the row key, so two runs of the same program align
by that attribute, not by transaction id.

*Statistics*: IPC = retired transactions / cycles in window; flush rate =
count of `status == aborted`; instruction classes by the label regexes.
All computed from data, none stored.

### 2.5 What changed versus a Kanata log

| Kanata feature | VTR data | VDB semantic |
|---|---|---|
| `I id gid tid` | transaction, stream `thread<tid>`, attribute `insn_id_in_sim` | row key |
| `L 0/1/2` | attributes `label`/`detail`, stage attribute `label` | which text goes where |
| `S`/`E lane stage` | stages with lane and name | colour, role, lane order |
| `R rid type` | end time, `retire_id`, status retired/aborted | flush overlay, hide toggle |
| `W consumer producer type` | relation `wakeup` with `type` | arrow anchors and style |
| `C=`/`C` | absolute cycles (`time.unit = cycle`) | time axis formatting |
| stage name `X` means execute | nothing | `role: execute` |
| stage names `f`/`stl` are stalls | nothing | `role: stall` |
| hue from lane/stage order | nothing | explicit colours or `auto` |

The Konata-specific name conventions (`X`, `stl`) have moved out of the
data and into the VDB, where they can differ per core generation.

### 2.6 Extending the viewer without touching the trace

* A second VDB for the same file can colour by `thread`, group rows by
  the `pc` attribute, or hide the `stl` lane: only the document changes.
* A SystemC/TLM trace (FTR converted, or written natively) uses the same
  viewer with a VDB whose `stages` map the `parent_of` relation into
  nested rows and whose `dependencies` use kind `pred`.
* An OpenTelemetry trace uses generators (span names) as rows, `otel.link`
  relations as arrows and `status == error` as the overlay.

## 3. Outline: an RTL-debugger VDB

The waveform side of VTR carries the elaborated hierarchy (scope and var
types with FST numbering, directions, widths, aliases, enum tables and any
attributes the simulator recorded, e.g. `fst.source_stem`). A debugger VDB
adds:

1. **Source links**: selectors on scope paths and var paths to
   `{file, line}`; when the simulator emitted `fst.source_stem`
   attributes the VDB can be generated from them, otherwise from the
   elaboration database of the tool.
2. **Driver/load annotations**: for each var path, the list of driver
   paths and load paths (static connectivity from the netlist). Reverse
   debugging then walks: value at time `t` of `x` (`value_at`), find the
   last change of `x` before `t` (`changes` in a small window walking
   backwards block by block), and for each driver of `x` repeat. All
   queries are local; no signal needs to be loaded fully.
3. **Formatting**: radix, signedness, enum table binding by var path
   (VTR carries `enum_table` node references from FST enum refs; the VDB
   may also add its own), struct/array grouping by naming pattern.
4. **Waveform view**: signal groups, colours, analog rendering hints,
   markers; the view lists var paths and uses `load_signals` for the
   visible set and `time_table()` for cursor snapping.
5. **Trigger/search**: named conditions (`posedge clk && req && !ack`)
   evaluated by loading the referenced signals once and walking their
   change lists; the VDB stores the expressions, VTR provides the data.

The VDB is keyed by var path, so it survives re-simulation and even
re-elaboration as long as names do not change; after a hierarchy change
the unresolved selectors are reported and the rest keep working.

## 4. Checklist for VDB authors

* Attach by names (paths, keys, kinds), never by ids.
* Treat unknown attributes, stages and relation kinds as data to ignore,
  not errors.
* Read the file's `time.unit` / timescale attributes before formatting
  time.
* Prefer `visit_transactions` with a window and generator over scanning;
  prefer `value_at` / `changes` over loading whole signals unless the
  signal is on screen.
* Keep the VDB in your own format and versioning; VTR's format version
  only constrains the trace.

## 5. Implemented RTL companion

[RTL VDB and temporal tracing](VDB_RTL.md) implements the source/elaboration
side using slang and provides a Rust CLI for time-aware driver trees. Unlike
a last-value-change walk, it follows assignment-triggering clock/reset events,
including unchanged register writes and enable holds. Its JSON schema is
independent of the illustrative pipeline-viewer configuration above.
