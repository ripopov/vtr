# Activity scan admission

The Rust activity builder uses block cost estimates to schedule parallel scans.
An estimate larger than the available workspace reserves the entire workspace
and runs alone. It does not reject the recording before scanning. Temporary
decoding buffers and retained activity vectors are checked before allocation;
vector growth uses the remaining reservation when ordinary doubling would exceed
it. Genuine exhaustion returns a memory-limit error.

Canonical columns are decoded once. Dynamic aliases append signal rows pointing
at the canonical row's immutable range slice, including its logical change
count. Shared ranges preserve per-signal statistics and cross-block stitching
without retaining duplicate activity or decoding the target again. Initial
values, repeated timestamps, and adjacent integer timestamps retain their usual
activity semantics.

The scheduler distinguishes running scan reservations, completed scans' actual
retained capacities, and encoder allocations. Completion immediately replaces a
scan's estimate with its retained capacity. An owned permit keeps that capacity
charged until the result's buffers are freed, including queued results dropped
on cancellation or error.

The encoder acquires capacity through the shared budget before allocating. The
admission lock protects accounting updates and is released during scanning,
encoding, compression, allocation, and output writes. If encoder growth needs
space held by running scans, new scan admission pauses while those scans finish
using their existing reservations. Waiting releases the lock; completed results
remain charged at their actual size. Scan workers never wait for extra memory
while holding partially built scans.

Reusable encoder buffers retain capacity when memory permits. Under pressure,
the encoder drops obsolete packed buffers, shrinks columns to their live contents,
and retries the required allocations without geometric spare capacity. This
prevents unused capacity from consuming space needed for subsequent packing.
Retained buffers are also released when they would prevent an otherwise idle
builder from admitting the next block. Encoder reservations release automatically
after their buffers are freed.

Capacity checks for range encoding are amortized: spare capacity is converted
into a guaranteed number of three-varint writes using the maximum ten-byte
varint width. Individual writes resume exact admission when that allowance runs
out. No counting or second decompression pass is required.

`BuildOptions::memory` is an activity table and buffer budget. It excludes the
already-open reader, codec contexts, allocator overhead, and thread stacks; it
is not a process RSS cap. Generator interval collection retains its separate
bounded union admission. Neither VTR nor VTI bytes changed.

## Validation

Regression tests cover dense dynamic aliases across groups, shared coalesced
ranges, oversized scheduling estimates between ordinary blocks, exclusive-scan
progress and actual exhaustion, wide values with little activity, and timestamps
through `u64::MAX`. The existing full Rust suite, all 40 native tests including
Rust/C++ interoperability, and Volna's 24 activity integration tests pass.

A dense-alias regression spans 20 blocks, 256 signals, and 7,679,744 logical
changes. At a 6 MiB workspace budget it produces identical sidecars with one,
four, and eight workers. A blocking-output test waits for additional worker
completion and admission during the first block's write, checking that output
does not hold the scheduler lock. Real exhaustion, cancellation, and transitions
between parallel and exclusive scans are tested separately.

A 395-signal GPU recording previously rejected a 512 MiB build budget because
56 sparse aliases were charged at every one of 430,332 block timestamps. Their
375 actual entries were budgeted as 24,098,592 points. The recording now builds
with both 512 MiB and 32 MiB workspace budgets. The latter exercises exclusive
admission. Both produce the same 159,303-byte sidecar as the baseline build,
with 864,267 changes.

## Activity build timing

Release-mode native macOS measurements compare the implementation against
`eec7eee353ccd16ed66bb5546619633ec80db425`. Each case uses eleven shuffled
before/after pairs, one scan worker, a 1 GiB admission budget, and identical
recordings. Timing starts after opening the reader and includes building and
writing the index; subsequent decoding and source verification are excluded.
Every paired sidecar is byte-identical. These are local microbenchmarks, not
cross-platform throughput guarantees.

| Recording | Baseline median | Candidate median | Change |
|---|---:|---:|---:|
| GPU recording | 76.020 ms | 75.213 ms | -1.1% |
| 64 distinct 64-bit signals, 30,000 steps | 26.915 ms | 25.984 ms | -3.5% |
| 64 identical 64-bit signals, 12,000 steps | 11.884 ms | 9.849 ms | -17.1% |
| 256 signals, one dense and 255 sparse identical columns | 2.111 ms | 1.963 ms | -7.0% |

Synthetic timestamps are three ticks apart. Distinct signals use different
counter increments; dense aliases share the same counter. Sparse signals change
once per 10,000 steps over 100,000 steps. Synthetic waveform blocks flush at
400,000 entries. No slowdown was observed in these cases. Small timing
differences on this host do not establish a general speedup; alias sharing
removes duplicate decoding and range construction where columns repeat.

## Default benchmark comparison

The complete default suite was rebuilt and run in the existing Linux container,
pinned to CPU 0, with three repetitions and full workload sizes. It completed
in 67.24 seconds. All 490 shared metric cells were
compared with the previously committed [report](../benchmarks/report.md),
including every backend, workload, producer cost, read/query latency, RSS, and
size metric. All 164 deterministic size, count, and query-result cells match.

The default suite benchmarks the unchanged native C++ implementation and does
not build Rust activity indexes. VTR write-time differences against the previous
report range from -2.6% to +10.1%, and full-read differences from -7.4% to +24.6%.
The executable's SHA-256 remains
`41192afaaf99ae622a5f40cf606e33f4015a047ad1a0952c0bf98f6e73efc632`,
identical to the earlier validated run and its seven-pair identical-binary
control. That control's write/read differences range from -4.4% to +0.8%; close
times vary by up to 33.3%. The full suite's many-generator close median was
3.14 ms versus the previous report's 0.37 ms; the identical-binary control
measured 0.34 and 0.36 ms. Default-suite differences with the same executable
reflect measurement conditions rather than this Rust-only change.
