# Volna example traces

## Feature showcase

`feature_showcase.vtr` is a deterministic, synthetic debugging lab: 2,048 ns of
CPU execution, DMA traffic, an injected fault and recovery. It is **5,933 bytes**,
well below the 500,000-byte limit enforced by its generator. It contains 80
variable declarations sharing 77 signals, 17,430 changes, 98 transactions
(including 20 log records), and 59 relations.

Start at `soc`: add `clk`, `reset_n`, `valid`, `ready`, `address`, `data`,
`state`, `temperature_c` and `phase`. The fault window is 768–896 ns;
`ready` becomes X and `data` contains X/Z bits. There is a deliberate recording
gap at 960–992 ns. Search everywhere for `read`, `instructions` or `cycle` to
exercise mixed hierarchy results. `soc.log` demonstrates severity badges and
source locations. All activity and log locations are synthetic.

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

Volna currently displays waveforms and browses streams, generators and log sites.
Transaction/log panels and enum-name translation remain future work; use the
CLI or reader APIs to inspect those recorded details. The writer clips an
unfinished stage to capture end while preserving its transaction's `Open` status.

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
