# GPU instruction trace benchmark

The default benchmark suite includes `gpu-instructions` (LZ4) and
`gpu-instructions-zstd` (Zstandard level 1). They exercise short, frequently
repeated instruction descriptions alongside dynamic payloads and embedded
detail. This complements the [large inline-map comparison](../benchmarks/intern-values.md):
moving fields out of an existing typed payload has different costs from
replacing a repeated tagged object.

Each trace contains 262,144 instructions across 128 wave sources, repeating
50 descriptions. Both representations contain the same address, machine code,
mnemonic, opcode category, scalar/vector category, atomic result, encoded size,
ISA target and pipeline route. Sequence number, viewer label, issue status,
execution mask and resolved memory segment remain in the instruction payload.
Each instruction also has five embedded phases and a directed ownership link
from its wave. Memory instructions carry an enum-valued phase attribute.

`vtr-payload` stores all instruction fields in a typed payload.
`vtr-interned` stores static fields in a typed map once and attaches its
`ValueId` as the `instruction` attribute. The producer builds descriptions only
once and reuses the IDs. This is a standalone synthetic workload; it does not
run a GPU model or predict simulation runtime.

Each codec runs three batching policies for both representations:

| Suffix | Block limit | Purpose |
| --- | --- | --- |
| None | Default 512 KiB, 65,536 items | Library defaults |
| `-batched` | 2 MiB, 65,536 items | Measure a larger bounded capture batch |
| `-fixed` | 2,048 items, nonbinding 64 MiB byte limit | Keep block membership equal between representations |

An item is a transaction or relation. All policies keep the default 64 KiB
decompressed page target; larger capture batches do not require a query to
decompress an entire block. The report includes block counts and peak capture
buffer accounting so file-size changes can be assessed alongside buffering.

Writes include metadata construction, capture, encoding, compression and close.
Reads open a fresh reader, verify every instruction and phase, then query all
128 waves' outgoing ownership relations. They check the generated schedule's
IDs, times, parentage, static fields, dynamic masks, phase names and statuses,
and relationship endpoints. These timings include verification and relationship
queries. The null producer computes a schedule checksum; it does not model
phase capture costs. For these profiles, one producer tick is one instruction.

Run the full suite on Linux from the repository root:

```sh
python3 benchmarks/build.py --offline
python3 benchmarks/run.py
python3 -m unittest discover -s benchmarks -p 'test_*.py'
```

Omit `--offline` only when the pinned benchmark dependencies have not yet been
fetched. To run only these workloads, pass
`--profiles gpu-instructions gpu-instructions-zstd` to `run.py`. Results are
included in the [suite report](../benchmarks/report.md); its results directory
also contains raw measurements, independent expected digests and binary hashes.

Interning reduces repeated decoded content, but smaller uncompressed streams
can still compress worse. References add attribute-list framing, and memory
accounting can cause an earlier batch flush. Compare identical metadata,
hold block membership fixed to isolate encoding effects, then evaluate the
codec and batch size appropriate for the producer.

Reference writers place ownership relations on separate pages. When relations
share a page with many instruction and phase attributes, an endpoint query must
skip those attribute streams before reaching the relations. The benchmark's
128 wave queries expose this repeated work, including when the instruction
data itself compresses well. Separate pages preserve the existing format and
allow the reader to select relationship data independently.

On native Linux AArch64, three paired runs of the interned representation
compared the mixed-page writer at `b1a68f3` with separate relation pages. Both
executables used the same benchmark source, default batch limits and selected
codec. Read time includes instruction verification and all 128 ownership queries.

| Codec | File bytes before | File bytes after | Read before (ms) | Read after (ms) |
| --- | ---: | ---: | ---: | ---: |
| LZ4 | 3,893,150 | 3,936,111 | 2,660.93 | 529.15 |
| Zstandard level 1 | 2,507,661 | 2,425,245 | 3,023.95 | 729.89 |

The LZ4 size cost is 1.1%; Zstandard becomes 3.3% smaller. The complete
[paired report](../benchmarks/gpu-regression/report.md) retains every workload
and metric, with [raw paired observations](../benchmarks/gpu-regression/paired.json).
The [GPU workload observations](../benchmarks/gpu-regression/workloads.json)
include every representation, codec and batch policy in the default suite.
Write timings have overlapping ranges and vary substantially between runs;
these measurements do not establish a writer speedup.

A [15-pair write probe](../benchmarks/gpu-regression/write-pairs.json) investigates
that timing variation with randomized variant order. Across the two codecs and
default/2 MiB batches, median within-pair changes range from −0.5% to +2.1%,
with overlapping run ranges. It does not reproduce a large writer regression.
The retained data includes the runner source and executable hashes.
