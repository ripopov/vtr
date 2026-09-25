# Volna example traces

## Pipeline showcase

`pipeline_showcase.vtr` exercises the pipeline panel: two synthetic cores run
the same twelve-instruction loop and are recorded as `PIPELINE` streams
(`soc.cpu0.pipeline` and `soc.cpu1.pipeline`, generator `instruction`) on a
cycle time base (`time.unit = cycle`, timescale 0). `cpu0` is a five-stage
in-order core (`F D X M W`); `cpu1` is a two-wide eight-stage core
(`F Dc Rn Ds Is Rr X Cm`). Every instruction carries `vtr.label` (`pc: mnemonic
operands`), `insn_id`, `pc` and `iteration` attributes and its stage cells on
lane `0`. Cache misses extend the memory stage (`miss` stage attribute) and stall
younger instructions, drawn as `stl` overlays on lane `stall` with a `reason`;
every fifth (`cpu0`) or seventh (`cpu1`) branch is mispredicted and the younger
fetched instructions are flushed (`Aborted`). Register read-after-write
dependencies are `wakeup` relations from producer to consumer; loads and stores
issue `soc.l2.bus` requests (`read`, `write`) parented to the instruction and
linked by `causes`. The recording stops while several instructions of each core
are in flight, which stay `Open`. Waveforms per core: `pc`, `fetch_valid`,
`stall`, `flush` and `retired`.

Open it, double-click either `pipeline` stream (or use View ▸ Pipeline), then
add `soc.cpu0.stall` to the wave panel and zoom into a stall burst: the cursor
and viewport stay linked across the panels.

Regenerate from the repository root:

```sh
cargo run --locked -p vtr --example pipeline_showcase -- \
  volna/volna/examples/pipeline_showcase.vtr
```

The [generator](../../../core/vtr/examples/pipeline_showcase.rs) simulates
both cores deterministically (a fixed-seed xorshift decides misses), writes
the waveforms then the transactions, reopens the file and checks the counts,
statuses and size (below 250,000 bytes). Regenerate rather than preserve an
older encoding when the writer changes.

## Feature showcase

`feature_showcase.vtr` is a deterministic, synthetic debugging lab: 2,048 ns of
CPU execution, DMA traffic, an injected fault and recovery. It is **7,419 bytes**,
well below the 500,000-byte limit enforced by its generator. It contains 81
variable declarations sharing 78 signals, 17,559 changes, 98 transactions
(including 20 log records), and 59 relations.

Start at `soc`: add `clk`, `reset_n`, `valid`, `ready`, `address`, `data`,
`state`, `temperature_c` and `phase`. The fault window is 768–896 ns;
`ready` becomes X and `data` contains X/Z bits. There is a deliberate recording
gap at 960–992 ns. Search everywhere for `read`, `instructions` or `cycle` to
exercise mixed hierarchy results. `soc.log` demonstrates severity badges and
source locations. `hotplug_sensor` is declared at 1,024 ns, after half of the
values were written: its `reading` signal is X before then. All activity and
log locations are synthetic.

Stream kinds describe their domain: `PIPELINE` for instruction execution,
`MEMORY_BUS` for bus requests, `LOG` for messages, and `otel.scope` for software
spans. Both `soc.dma.memory_bus` and the empty `soc.dma.standby_bus` use
`MEMORY_BUS`: activity does not change what kind of stream they are.

| Area | Contents |
| --- | --- |
| Waveforms | 2-, 4- and 9-state logic; scalar and 32-/128-bit buses; every IEEE 1164 state; reals; UTF-8 text and arbitrary bytes; enum table references; constants; events with repeated occurrences at one timestamp; three aliases |
| Hierarchy | Modules, CPU core, SystemC module, resource, streams and generators; every named scope type and variable type in `soc.type_gallery`; every direction; unknown scope/variable codes; literal dots and brackets in names; empty stream and empty generator |
| Transactions | Overlapping instructions, speculative squashes, read/write transfers, software spans; every status and span kind; begin/record/end attributes; timestamped events; stages on `main` and `memory` lanes; unfinished operation at capture end |
| Links | Cross-stream request relations, instruction dependencies, structural span/transfer parents and logs parented to transfers; attributes on relations, events and stages |
| Attributes | All 18 `ValueTag` variants, including 2-/4-/9-state vectors, signed/unsigned fixed point, pointers, nested lists/maps, interned strings and inline Unicode text |
| Logs | All six standard severities plus a producer-specific level; all nine log argument types; argument names; file, line and function provenance |
| Metadata/container | Nanosecond timescale, −16 ns time-zero offset, fixed producer/date/comment, file attributes, mixed-language file type, dump-off/on markers, multiple waveform blocks, late hierarchy declarations, checksums and final directory |

The main design is intentionally small; the declaration gallery is a type
catalog, not a model of real hardware. Trace times above are raw ticks, before
the time-zero offset. Presentation colours, source mappings for HDL, and viewer
layout are not embedded in VTR.

Volna displays waveforms, browses streams, generators and log sites, and shows
any stream or generator as a pipeline panel (`soc.cpu.thread0` here, and the
`memory_bus` requests as stage-less cells). Log panels and enum-name translation
remain future work; use the CLI or reader APIs to inspect those recorded
details. The writer clips an unfinished stage to capture end while preserving
its transaction's `Open` status.

This is a **data-model showcase**, not an exhaustive encoding or corruption-test
corpus. It uses default compression; alternative codecs and crash recovery are
not represented. Late hierarchy growth adds a scope and an alias after a flush.
New signal IDs after a waveform flush are deliberately excluded: the current
reader indexes the earlier block's shorter initial-value table when loading
such a signal (`Reader::load_signals`), causing an out-of-bounds panic.

Regenerate from the repository root:

```sh
cargo run --locked -p vtr --example feature_showcase -- \
  volna/volna/examples/feature_showcase.vtr
```

The [generator](../../../core/vtr/examples/feature_showcase.rs) uses fixed inputs
and synchronous writing. It reopens the result, decodes every signal, transaction,
relation and log, checks feature coverage, and rejects files of 500,000 bytes or
larger. With the pinned dependencies, repeated generation produces identical
bytes. The file can change when the writer or format changes; regenerate rather
than preserving compatibility with an older encoding.

Inspect or open it:

```sh
cargo run -p vtr-cli --bin vtr -- info volna/volna/examples/feature_showcase.vtr
cargo run -p vtr-cli --bin vtr -- log volna/volna/examples/feature_showcase.vtr --severity warn
cargo run -p vtr-cli --bin vtr -- tx volna/volna/examples/feature_showcase.vtr --max 12
cargo run -p volna --profile viewer -- volna/volna/examples/feature_showcase.vtr
```

## Large FST stress trace

`large_fst.fst` (about 550 MB, not committed) is for manual performance
testing of FST loading and analog rows. Every signal samples once per 1 ns
tick: 32-bit signed sine waves `sine_1m`, `sine_10m` and `sine_100m` with 1M,
10M and 100M samples (periods of 1,000, 20,000 and 1,000,000 samples; each
holds its last value after its samples end) and a `clk` toggling on each of
100M ticks. Generate it from the repository root (about 7 s):

```sh
cc -O2 volna/volna/examples/generate_large_fst.c \
  ext/libfstwriter/integration_test/verilator_share/gtkwave/fstapi.c \
  -Iext/libfstwriter/integration_test/verilator_share/gtkwave \
  $(pkg-config --cflags --libs liblz4 zlib) -lm -o /tmp/generate-large-fst
/tmp/generate-large-fst volna/volna/examples/large_fst.fst
cargo run -p volna --profile viewer -- volna/volna/examples/large_fst.fst
```
