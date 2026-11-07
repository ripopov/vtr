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
Rust/C++ interoperability, and Volna's 30 activity integration tests pass.

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

## Index loading estimates

`Index::estimate_memory` bounds vector capacities from section row counts before
allocating signal tables or decoding pages. Retained storage and loading peak
are reported separately; the peak also includes encoded input, directory storage,
per-signal scratch, and the largest decoded page. Codec contexts and allocator
overhead remain outside these estimates. Full decoding still validates rows and
performs memory admission. `Index::validate_with` shares that decoder while
omitting retained signal stretches and keeping only the last interval per
generator. This allows sidecar validation and cache fallback before transporting
an index whose full decoded representation exceeds the automatic loading limit.

Waveform and generator query tests check retained estimates against decoded
storage. Admission tests check that the estimated peak covers every allocation
request. Volna integration tests exercise approval, dismissal, budget growth,
refused admission and retry, configured object limits, invalid-cache recovery,
and approval of remote sidecars larger than the original transfer allowance. A native UI test clicks the
approval button and verifies that the index becomes available.

## Default benchmark comparison

The complete default suite was rebuilt and run in the Linux container on CPU 0,
with three repetitions and full workload sizes. It completed in 52.67 seconds.
All 490 shared metric cells were compared with the previously committed report,
including producer costs, read/query latency, RSS, and sizes for every backend
and workload. All 164 deterministic size, count, and query-result cells match.

These benchmarks compile native C++, whose inputs are unchanged by the Rust
estimation API. They do not measure Rust activity-index loading. The rebuilt
native executable's SHA-256 is
`bd05d726a27b0df09c7bf7b1df899eaae279001fdb45e74096d6fbfbd76e5691`;
it differs from the executable recorded in the previous validation, so timing
and RSS differences against that report cannot isolate this API change.
A seven-pair identical-executable control used full workloads on CPU 0: write
medians varied from -0.6% to +3.0%, read medians from -0.7% to +1.9%, and close
medians from -3.9% to +2.4%. Every control result matched its source replay.
