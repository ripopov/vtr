# VTR 3 reference evaluation

The version-3 model now covers the semantic goals in [RATIONALE.md](RATIONALE.md): reusable aggregate payloads, stable identities across owner/location views, distinct repeated occurrences, explicit ownership/causality, late declarations, clocks, diagnostics and honest recovery. The C++20 reference validates these behaviors. **The performance goals are only partly met.** Waveform writes beat FST in the measured common workloads and approach or beat VCD, while transaction writes still trail FTR and current VTR. Producer capture remains a substantial part of tracing CPU; the most-off-main target is not demonstrated across the measured workloads. This proposal is ready for model/encoding review, with performance remaining an adoption gate.

[TRANSACTIONS.md](TRANSACTIONS.md) specifies the implemented format. Production remains VTR 2; this evaluation does not change production readers, writers or assets. The reference and raw research artifacts live under `atemp/vtr3-reference` in the Volna workspace and are intentionally untracked.

## Scope and method

Post-run host inspection found two stale Volna unit-test processes, each running for over 22 hours, with six spinning threads in total allowed on CPUs 0–19. Their lifetimes covered this study; they were stopped after measurement. Shuffled paired runs share that background load but do not establish idle-host throughput. Absolute times and small differences require confirmation on an idle host.

Measurements use an Intel Core Ultra 7 265K (20 physical cores, one thread per core), Linux 7.0.0-38 and GCC 15.2.0, C++20 `-O3 -DNDEBUG`. Single-CPU comparisons pin the complete process to CPU 0. Worker comparisons use the same shared CPU set for synchronous and asynchronous variants: either `{0}` or `{0,1,2}`. Two workers are additional threads inside that budget. Order is shuffled deterministically.

The six existing default workloads are unchanged. Seven paired complete old/new trials check independent producer replays, complete reads and all twelve probes of every applicable query. Separate complete default suites use three repetitions across VTR/FST/VCD, VTR/FTR and paired mixed backends. Worker studies use five repetitions. The five new location/navigation workloads use three repetitions under each CPU budget. Every timed read and query matches its source-replay digest; all observations enter count, sum and XOR checks.

Writes include generation, capture, synchronization, encoding, compression, checksums, metadata/indexing, final drain and close. Capture is the whole producer loop, including backpressure. Close includes remaining encoding/output. Process CPU includes workers; producer-thread CPU is also reported for VTR 3. A counting null sink follows the same generation/lifetime schedule. RSS is a fresh executable’s `/proc/self/status` VmHWM, with maxima across repetitions. Buffered I/O is used; fsync, cold disk/remote I/O and power-loss durability are not measured. OS cache is warm after writing.

Both VTR implementations and the other binary backends select fast LZ4 (`LZ4_compress_default`). The VTR `compression_level=3` option in some archived labels does not affect LZ4; it affects Zstandard only. Stored fallback remains enabled. The reference API defaults to Zstandard level 1, so these are controlled fast-LZ4 comparisons rather than timings of every default codec. Stored/LZ4/Zstandard correctness is tested independently. Dependency revisions are libfst `17f6a5d27b2f576d65b550114fbde3c035cc74a8`, LWTR `8dca86925ad64b0ca73825fcefd7308215049627`, and unordered_dense `ddd49116e63be7345b448351cad153653a712fdb`.

Queries use the named implementations and adapters, not an assumed format limit. Every cold query opens and destroys a reader, charging declarations/header preparation. Warm navigation opens once and reuses available caches; its open cost is separate. VTR selective timing uses default lazy CRC checking; full section integrity validation is a separate correctness gate. A section-wide CRC can require reading other compressed pages even when only selected pages are decompressed. p95 is nearest-rank over all probes, not a confidence interval.

## Existing workload results

The table uses seven paired single-CPU trials. RSS is the largest writer peak. Full metric tables, including every read/query latency, count, RSS and producer cost, follow in the appendices.

| Workload | Write V2 / V3 ms | Write change | Read V2 / V3 ms | File V2 / V3 MiB | Writer RSS V2 / V3 MiB |
|---|---|---|---|---|---|
| core | 112.32 / 124.78 | +11.1% | 41.23 / 36.87 | 7.666 / 7.695 | 51.75 / 18.96 |
| soc | 366.34 / 459.56 | +25.4% | 105.87 / 110.85 | 24.806 / 24.579 | 114.37 / 16.21 |
| idle | 49.82 / 63.35 | +27.1% | 15.18 / 15.51 | 1.853 / 1.733 | 22.27 / 15.43 |
| requests | 30.09 / 47.48 | +57.8% | 16.53 / 13.28 | 2.292 / 1.616 | 9.77 / 12.03 |
| many-generators | 55.96 / 84.45 | +50.9% | 29.34 / 21.35 | 3.775 / 2.569 | 9.37 / 12.01 |
| mixed | 36.00 / 49.57 | +37.7% | 14.95 / 12.41 | 2.500 / 2.192 | 17.29 / 20.00 |

Paged schema-guided transaction storage reduces the existing transaction files by about 30–32% versus VTR 2 and makes bounded transaction searches much faster. Whole-trace value searches still reconstruct many records; their improvement is smaller and they can trail FTR. These query/size wins do not erase the additional capture/serialization cost. Waveform batching deliberately trades some write time and compression for much lower writer memory than the old large record-only buffers.

| Waveforms | V3 write ms | FST write ms | VCD write ms | V3 / FST MiB |
|---|---|---|---|---|
| core | 124.56 | 692.39 | 168.18 | 7.695 / 8.941 |
| soc | 456.95 | 840.88 | 516.71 | 24.579 / 52.345 |
| idle | 63.04 | 115.37 | 67.06 | 1.733 / 4.165 |

| Transactions | V3 write ms | FTR write ms | V3 time change | V3 / FTR MiB |
|---|---|---|---|---|
| requests | 47.65 | 39.49 | +20.7% | 1.616 / 4.424 |
| many-generators | 83.69 | 67.24 | +24.5% | 2.569 / 7.571 |

The waveform target is demonstrated only for these two-state, at-most-64-bit synthetic observations. Wide/multistate vectors, real values, byte values, event duplicates, late signals and blackouts have correctness coverage, not comparable throughput evidence. The C910/SystemC/GPU rates in the rationale are not calibrated by these synthetic cases.

## Worker budget and capture

Worker mode owns completed batches and immutable declarations, bounds queued/encoding/ready/writing work, writes in submission order and exposes wait/occupancy counters. It does not depend on mutable simulator state. Signal spans batch synchronization by time step; owning final payload/end calls avoid another copy/lock. Typed `end_value` keeps serialization inline and uses bounded recycled storage. Synchronous measurements use scalar signal calls because forcing batches can add cost there; the source facts are identical. Workers still leave normalization, deduplication, time-index header packing, payload capture/validation and current-state updates on the producer.

| Shared CPUs 0,1,2 | Sync / 2-worker wall ms | Worker capture / close ms | Total / producer CPU ms | Estimated tracing CPU on producer | Worker RSS MiB |
|---|---|---|---|---|---|
| core | 124.04 / 108.30 | 106.30 / 1.82 | 122.95 / 101.29 | 70.7% | 25.50 |
| soc | 457.36 / 373.78 | 363.87 / 8.45 | 507.67 / 366.92 | 61.5% | 24.78 |
| idle | 63.04 / 59.64 | 48.74 / 9.47 | 70.46 / 51.51 | 63.3% | 20.24 |
| requests | 47.34 / 47.34 | 42.38 / 4.34 | 65.74 / 35.41 | 47.3% | 22.92 |
| many-generators | 84.65 / 66.94 | 63.09 / 4.06 | 106.65 / 57.74 | 46.8% | 22.10 |
| mixed | 49.73 / 51.63 | 42.63 / 8.58 | 58.39 / 40.65 | 61.0% | 26.85 |

The transaction-only workloads put about 47% of estimated tracing CPU on the producer; waveform/mixed workloads put 61–71% there. Worker mode improves high-generator transaction wall time under the three-CPU budget, while requests are approximately unchanged and mixed has a small cost. The median peak number of simultaneous encoders is one in these default runs: conservative charges can restrict concurrency before the job-count limit. The larger-queue trial below did not justify doubling the default budget.

The producer share subtracts the null process CPU from producer and total CPU as an estimate of tracing overhead; it is not a per-function profiler. The most-off-main target must hold across workloads, rather than only a favorable codec or payload case. Worker gains must include total CPU, backpressure and final drain; extra CPU is an explicit policy choice. The bounded recycler reduces worker capture allocations, but does not establish real producer capacity or remove the remaining producer costs.

The default waveform target is 16 MiB of logical captured changes/time entries, plus one observation when crossing the target. The record/TX source target is 4 MiB with at most 65,536 completed records, and raw pages target 64 KiB. Queues admit 8 jobs and 256 MiB of conservative retained-job charges. A single oversized job runs exclusively; the limit is not a hard RSS ceiling. Active owners, pending embedded detail, distinct block locators, declarations, signal current state, codec workspace and caller-held query results have separate lifetimes. Per-owner locators still grow with touched blocks; a long-lived owner is not constant-memory.

Payload and completed-row storage reuse defaults to a 16 MiB retained-capacity budget with workers and zero synchronously. Half serves source-local spares and half serves worker returns, including cached vector metadata. Input is recycled only after encoding has produced independent owning output; its queue charge remains until writing finishes. Active input/output, allocator overhead and codec workspace are outside the recycler budget. Seven paired trials found no clear synchronous benefit, so enabling it there requires an explicit budget.

A controlled earlier waveform study tested 4/16/64 MiB and no byte target, five complete repeats with all reads/probes. The 4 MiB target reduced memory most but added 42% to idle-file size and substantially slowed SoC/idle writes. At 16 MiB, that idle file was smaller than V2 while writer memory stayed below the old large-buffer policy. At 64 MiB, conservative queue charges exceeded 256 MiB for individual waveform jobs, forcing exclusive oversized admission. No byte target left source memory dependent on record volume. The 16 MiB default is a balance, not an optimum for every trace; all raw policies and query metrics are retained in the batching-study reports.

## Located records and complete navigation

The new benchmark represents the same IDs, parents, intervals and numeric/text values in both versions. V2 stores located stages/events as ordinary transactions with a location generator and owner parent; V3 stores native STAGE/EVENT records. V2 uses numeric attributes where V3 additionally declares integer widths. Both retain duplicate coincident events and overlapping visits. Ordinary children, owner labels, logs, outgoing causal relations and a clock stretch are included. Unlocated globally identified records, open embedded ends and multiple paused/frequency-changing clocks are separate capability tests, outside the equivalent old-format subset.

Balanced and disordered use 20,000 owners; disordered permutes timestamps independently of emission. Longtail attaches 30,000 families of completed visits/events/logs to one dispatch that stays active throughout. Logburst uses 10,000 owners and 16 logs each. Large uses 2,000 owners with ragged 4–4.25 KiB text. Every family has two overlapping visits at one location, two identical coincident events and an ordinary child every sixteenth family. Expansion, ordinary-child and log queries own all matching results on both sides before checksumming; V2 callback results are materialized to match V3 owning APIs. Streaming those results away would undercharge viewer memory/work.

| One CPU | Write V2 / V3 ms | File V2 / V3 MiB | Cold track V2 / V3 ms | Track+owner values/labels V2 / V3 ms | Expansion V2 / V3 ms |
|---|---|---|---|---|---|
| balanced | 15.21 / 27.42 | 0.408 / 0.486 | 3.580 / 0.051 | 5.851 / 1.531 | 8.457 / 1.342 |
| longtail | 18.44 / 22.13 | 0.918 / 0.546 | 2.388 / 0.050 | 3.667 / 0.070 | 22.652 / 14.439 |
| logburst | 16.06 / 30.38 | 0.248 / 0.300 | 2.820 / 0.034 | 5.471 / 0.673 | 17.583 / 0.400 |
| disordered | 15.26 / 27.76 | 0.413 / 0.493 | 5.734 / 0.095 | 10.011 / 3.068 | 8.429 / 1.384 |
| large | 78.70 / 47.31 | 0.295 / 0.380 | 1.733 / 0.063 | 7.955 / 0.982 | 15.582 / 0.710 |

Location selection is much cheaper, but labels/owner values add significant lookup work and must be charged. A whole long-lived owner still requires decoding and retaining a large result: coarse block locators do not make that query cheap. V3 expansion nonetheless improves on the equivalent retained V2 result in these cases. Some located-record files are larger and small-payload capture is slower than V2; independent pages, typed declarations, locator/header work and active-stage bookkeeping have real costs. Large-payload write improvement does not generalize to small records. The appendices include cold/warm p50/p95, all result counts, CPU, open cost and peak memory for every navigation operation under both budgets.

## Optimization evidence and choices

End-to-end optimization studies use complete source/read/query oracles and seven shuffled full-workload pairs. Initial codec-only trials are labeled separately. Frozen executables, build hashes and source archives preserve the variants. These are incremental comparisons on the named revisions, not additive speedup estimates.

| Change | Evidence directory under experiments | Observed result / decision |
|---|---|---|
| Empty payload paths | Codec-only trials: 1.4 million records each | Encoding +28–36%, decoding +78–94%; not an end-to-end throughput claim |
| Bulk little-endian capture | scalar-capture-paired | Transaction write time reduced about 7–9% |
| Bulk scalar reconstruction | word-reconstruction-paired | Transaction full read reduced about 21%, whole value searches about 27% |
| Conditional checkpointing / one page-size calculation | page-assembly-paired | Transaction writes reduced about 9–10% |
| Bounded flat scalar programs | scalar-program-paired | Transaction writes reduced about 7%, reads 9–10%; aggregate fallback retained |
| Direct-width scalar loads | scalar-word-load-paired | Small mixed effects; retained for clearer direct loads, no broad speed claim |
| Remove primitive programs | scalar-no-primitive-program-paired | Rejected: full reads regressed 10–13% |
| Owned final payload/end | owned-end-paired | Avoids a copy/lock; only small end-to-end wins, not a throughput solution |
| Signal-update spans | capture-api-paired; batched-three-cpus | Useful for workers; forcing them synchronously can regress SoC capture |
| Deferred exact page framing scans | lazy-page-size-paired; lazy-worker-confirm | Sync writes improved 10–12%; always-deferred workers regressed high-generator writes 17% |
| Mode-selected framing scans | worker-exact-policy-paired | Retain deferred scans synchronously and exact scans for workers; removes the large worker regression without changing encoded bytes |
| Inline typed final capture and bounded flat recycler | reuse-visible-one-cpu; reuse-visible-three-cpus | No clear synchronous win; workers: requests write −15%, many-generators −27%, with lower RSS. Default reuse budget is 16 MiB only with workers |
| 512 MiB queue charge budget | queue-capacity-three-cpus | Rejected as default: requests improved about 4% with higher RSS; many-generators and other workloads changed little. Retain 256 MiB |
| Publish reusable buffers as whole completed batches | batch-reuse-three-cpus | Rejected: requests wall time regressed 8% despite lower producer CPU; retain flat per-buffer reuse |
| Reset owner slots only when displacing a live owner | reconciled-reference; reusable_capture_budget_and_lifetime | Finalization already clears free slots; preserve that invariant and test payload, attributes, status and span kind across slot wraparound |

The page-bound optimization uses a conservative row-size bound between exact framing scans. Exact scans remain in worker mode because repeated paired high-generator trials showed regressions with deferred scans. Across 108 ragged aggregate/page/codec/mode cases, both paths match the earlier exact-size encoder byte for byte and pass page-bound/full-block validation. Complete default traces also match under common capture/codec settings except the random recording ID.

Stage-only Rust prototype timings omit parts of this container/API and do not establish this C++ implementation’s complete write/navigation cost. Fine owner-to-page or exact-ID indexes and adaptive activity summaries require separate measured justification. Reusable storage is bounded and measured here; broader pooling is not assumed to be faster.

## Requirements and validation

| Rationale requirement | Result | Evidence / boundary |
|---|---|---|
| State versus occurrences; stable identity | Covered | Event signals and identical coincident records survive; global IDs include embedded detail; both views and relations resolve the same records |
| Physical/logical hierarchy; late sources | Covered | Independent generator/parent links, active-parent rules, ordinary children outliving parents, late types/generators under live workers |
| Typed reusable payloads | Covered | Shared immutable datatype DAG, structs/arrays/sequences/arrays of structs/enums, widths, float bits, packed logic and mandatory/deferred payloads |
| Ownership, causality, conflicts and lanes | Covered | Cross-owner edges, open embedded ends, repeats/overlap retained, no temporal-containment/exclusivity or presentation rule |
| Logging and clocks | Covered in core | Shared site/schema metadata, typed dynamic values, parent diagnostics, node-zero clock association, multiple domains/pauses/frequency changes |
| Incremental recovery and incomplete evidence | Covered | CRC/prefix scans, stable block numbers, dangling references reported, no fabricated owners/payloads or false normal finalization |
| Thread-safe owning bounded handoff | Covered | Serialized mutation calls, immutable snapshots, reverse completion, bounded queue states, backpressure, joined failure shutdown; borrowed metadata/type views need external serialization |
| Derived activity source binding | Covered for byte API | Recording identity + finalized length/directory CRC; stale/old sidecars rejected; exact threshold-one windows; cache publication/adaptive thresholds remain outside this API |
| Selective query cost | Demonstrated for measured workloads | Complete owner/location/label/log/child/relation/ID/clock probes, cold and warm, retained outputs and memory |
| FST / VCD waveform target | Demonstrated for common synthetic subset | Fast-LZ4 runs above; not calibrated real-design throughput |
| FTR transaction write target | Unmet | Current reference remains slower despite smaller files and faster selective queries |
| Most tracing work off the simulator thread | Not demonstrated across workloads | Worker codec/output and bounded reuse are implemented; measured producer CPU shares remain material |
| Real producer rate / capacity targets | Not established | Synthetic loops are insufficient to set C910, SystemC or GPU capacity budgets |

All **17 reference test groups pass in Release, ASan+UBSan and ThreadSanitizer builds** on the accepted source. Independent Python decoders check schema/record/TX wire layouts and stored/LZ4/Zstandard blobs. The independent production Rust waveform reader checks unchanged waveform section bodies after adapting only V3 header/directory framing; source-model tests cover 2/4/9 states, widths 1/13/96, real NaN/signed-zero bits, arbitrary bytes, aliases, repeated same-time events, late signals and blackouts. Full validation also rejects inconsistent frames, invalid logic codes/padding, bad predecessor/group partitions and directory bounds even when CRCs are valid.

Failure sweeps cover construction and producer operations in both worker modes, including owning/inline typed final payloads and batched signals. Recycler tests cover disabled/tiny budgets, oversized ragged payloads, source mutation, concurrent producers and owner-slot wraparound. Queue tests inject encoder/worker and sink exceptions; OS file-size limits force partial I/O failure. Valid written prefixes remain readable, incomplete provenance is retained and poisoned writers reject further normal recording/close. Held owning records survive cache eviction. LeakSanitizer is disabled because it cannot run in this environment; address/undefined-behavior checks remain enabled. Primary canonical payload reconstruction defaults to 64 MiB per record and the owner cache to 4,096 entries/32 MiB accounted data. Optional attributes, global metadata, caller-held references and total process RSS are separate limits/lifetimes.

Production was rebuilt from the VTR root and all **five native interoperability tests** plus the **full default benchmark suite** passed. All **490 committed metric cells** were compared; file sizes, normalization values and query counts are unchanged. The rebuilt production executable is byte-identical to the preserved baseline, and all production implementation/benchmark source hashes match. Timing and RSS variation was reviewed against repeated complete trials; no implementation regression is attributed to documentation changes. The updated production results are in [benchmarks/report.md](../../benchmarks/report.md).

## Provenance and reproduction

| Artifact | Revision / SHA-256 |
|---|---|
| Production VTR revision | `a0fa76252fc454ba7420009bf93c0a0d80e1eb6b` |
| Volna base revision | `fb042f0020785fd89e8fe373dcaf122e3e312170` |
| Production benchmark SHA-256 | `9512050cc6af8e670b0c8dd5211d72119aa29f66dba6854a7f7d2679e97f7f91` |
| Accepted V3 benchmark SHA-256 | `cefee5b389b341b15da7d8474eae399e17a7cc8f5f65570508b4ead5ff8dbdd0` |
| Accepted reference build-manifest SHA-256 | `760e0bc35d57b97a466dd386cbc9c3f1736241bf6b83a822c5490a8a545d00f8` |
| Delivery source archive SHA-256 | `8bc73b582f26cbb376e9cc7d2a40ff2b184b8bc53002e3d722d7125e1e7971b3` |
| Navigation source SHA-256 | `6c5c9b6d746a7f18c414e5cad9c11b08276b3aed8b6c5da52ccda80d6f688e56` |
| Navigation V2 executable SHA-256 | `15011965f07e6a6bd7799ec31ce0e17fff260e9d9629a692f74c122be63fe4b2` |
| Navigation V3 executable SHA-256 | `aed775d6ee22f5d92d9eeb827ff5a125c548b20a9e280ac968bf9236de887566` |

Local workspace artifacts (not committed) include `experiments/accepted-delivery-complete-source.tar.gz`, `reconciled-reference-{benchmark,build.json}`, complete `raw.json`/`report.md` files in every accepted study, `accepted-production/all-metric-diff.json`, `baseline/source.json`, `experiments/ragged-mode-equivalence.json`, `experiments/delivery-wire-equivalence/raw.json`, `validation.json`, and the release/sanitizer/native test logs. The build manifest records all 37 implementation/adapter source hashes; the delivery archive additionally contains tests, oracle sources, build scripts and navigation sources. The navigation manifest records compiler/flags, both linked library hashes and the reference manifest.

From the Volna root, build the temporary reference and run its tests:

```sh
cmake -S atemp/vtr3-reference -B atemp/vtr3-reference/build -DCMAKE_BUILD_TYPE=Release
cmake --build atemp/vtr3-reference/build --parallel
ctest --test-dir atemp/vtr3-reference/build --output-on-failure
python3 atemp/vtr3-reference/build_benchmarks.py
python3 atemp/vtr3-reference/benchmarks/run.py --cpu 0 --output atemp/vtr3-reference/experiments/new-default
python3 atemp/vtr3-reference/navigation/build.py
python3 atemp/vtr3-reference/navigation/run.py --cpus 0 --output atemp/vtr3-reference/experiments/new-navigation
```

Production dependency setup and the required production checks run from the VTR repository root:

```sh
cmake -S vtr-cpp -B build/cpp -DCMAKE_BUILD_TYPE=Release
cmake --build build/cpp --parallel
ctest --test-dir build/cpp --output-on-failure
python3 benchmarks/build.py
python3 benchmarks/run.py
```

New output directories must not exist. For worker studies, set `VTR3_WORKERS`, `VTR3_QUEUE_JOBS`, `VTR3_QUEUE_BYTES`, `VTR3_WAVE_BYTES`, `VTR3_PAYLOAD_POOL_BYTES` and optionally `VTR3_CAPTURE_BATCH`, and state the complete CPU affinity. `benchmarks/configurations.py` and `benchmarks/compare.py` preserve options, hashes and raw rows; no source change is required to select these policies.

## Appendix A: every paired old/new metric

### core

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 112.32 | 124.78 | +11.1% |
| Write throughput (Mobs/s) | 56.009 | 50.416 | -10.0% |
| File size (MiB) | 7.666 | 7.695 | +0.4% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.91 | 11.86 | -0.4% |
| Write peak RSS (MiB) | 51.75 | 18.96 | -63.4% |
| Capture time (ms) | 87.53 | 122.99 | +40.5% |
| Capture time (ns/tick) | 182.4 | 256.2 | +40.5% |
| File bytes per tick | 16.7 | 16.8 | +0.6% |
| Close time (ms) | 24.49 | 1.60 | -93.5% |
| Write CPU time (ms) | 113.25 | 125.64 | +10.9% |
| Write min / max (ms) | 111.36 / 114.09 | 123.07 / 125.48 | — |
| Read time (ms) | 41.23 | 36.87 | -10.6% |
| Read throughput (Mobs/s) | 152.587 | 170.649 | +11.8% |
| Read CPU time (ms) | 42.17 | 37.81 | -10.3% |
| Read peak RSS (MiB) | 17.79 | 14.03 | -21.1% |
| search-signal: CPU / 12 probes (ms) | 9.552 | 3.034 | -68.2% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 679 | 679 | +0.0% |
| search-signal: p50 (ms) | 0.446 | 0.112 | -74.9% |
| search-signal: p95 (ms) | 1.872 | 0.463 | -75.3% |
| search-signal: peak RSS (MiB) | 12.73 | 7.61 | -40.2% |
| find-signal: CPU / 12 probes (ms) | 18.835 | 14.445 | -23.3% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 2,493,463 | 2,493,463 | +0.0% |
| find-signal: p50 (ms) | 0.698 | 0.625 | -10.5% |
| find-signal: p95 (ms) | 6.156 | 5.069 | -17.7% |
| find-signal: peak RSS (MiB) | 12.72 | 10.18 | -20.0% |
| point: CPU / 12 probes (ms) | 11.231 | 3.817 | -66.0% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.518 | 0.120 | -76.8% |
| point: p95 (ms) | 2.282 | 0.759 | -66.7% |
| point: peak RSS (MiB) | 12.72 | 7.69 | -39.5% |
| Write producer CPU time (ms) | — | 125.64 | — |

### soc

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 366.34 | 459.56 | +25.4% |
| Write throughput (Mobs/s) | 54.987 | 43.833 | -20.3% |
| File size (MiB) | 24.806 | 24.579 | -0.9% |
| Bytes per observation | 1.29 | 1.28 | -0.8% |
| Logical bytes / file bytes | 11.78 | 11.89 | +0.9% |
| Write peak RSS (MiB) | 114.37 | 16.21 | -85.8% |
| Capture time (ms) | 349.51 | 450.92 | +29.0% |
| Capture time (ns/tick) | 14562.9 | 18788.3 | +29.0% |
| File bytes per tick | 1083.8 | 1073.9 | -0.9% |
| Close time (ms) | 15.58 | 7.24 | -53.5% |
| Write CPU time (ms) | 367.30 | 460.50 | +25.4% |
| Write min / max (ms) | 362.89 / 368.30 | 457.65 / 460.55 | — |
| Read time (ms) | 105.87 | 110.85 | +4.7% |
| Read throughput (Mobs/s) | 190.275 | 181.726 | -4.5% |
| Read CPU time (ms) | 106.81 | 111.81 | +4.7% |
| Read peak RSS (MiB) | 30.20 | 30.24 | +0.1% |
| search-signal: CPU / 12 probes (ms) | 4.503 | 4.033 | -10.4% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 693 | 693 | +0.0% |
| search-signal: p50 (ms) | 0.279 | 0.228 | -18.3% |
| search-signal: p95 (ms) | 0.473 | 0.483 | +2.1% |
| search-signal: peak RSS (MiB) | 6.45 | 6.64 | +2.9% |
| find-signal: CPU / 12 probes (ms) | 5.127 | 6.024 | +17.5% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 124,873 | 124,873 | +0.0% |
| find-signal: p50 (ms) | 0.315 | 0.387 | +22.9% |
| find-signal: p95 (ms) | 0.576 | 0.689 | +19.6% |
| find-signal: peak RSS (MiB) | 6.82 | 9.03 | +32.4% |
| point: CPU / 12 probes (ms) | 4.569 | 3.994 | -12.6% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.289 | 0.228 | -21.1% |
| point: p95 (ms) | 0.472 | 0.475 | +0.6% |
| point: peak RSS (MiB) | 6.50 | 6.68 | +2.8% |
| Write producer CPU time (ms) | — | 460.50 | — |

### idle

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 49.82 | 63.35 | +27.2% |
| Write throughput (Mobs/s) | 51.121 | 40.207 | -21.3% |
| File size (MiB) | 1.853 | 1.733 | -6.5% |
| Bytes per observation | 0.76 | 0.71 | -6.6% |
| Logical bytes / file bytes | 19.95 | 21.34 | +7.0% |
| Write peak RSS (MiB) | 22.27 | 15.43 | -30.7% |
| Capture time (ms) | 36.07 | 53.66 | +48.8% |
| Capture time (ns/tick) | 751.5 | 1117.9 | +48.8% |
| File bytes per tick | 40.5 | 37.8 | -6.7% |
| Close time (ms) | 12.57 | 8.32 | -33.8% |
| Write CPU time (ms) | 50.71 | 64.24 | +26.7% |
| Write min / max (ms) | 49.51 / 50.64 | 62.31 / 63.66 | — |
| Read time (ms) | 15.18 | 15.51 | +2.2% |
| Read throughput (Mobs/s) | 167.810 | 164.208 | -2.1% |
| Read CPU time (ms) | 16.14 | 16.44 | +1.9% |
| Read peak RSS (MiB) | 7.29 | 7.29 | +0.0% |
| search-signal: CPU / 12 probes (ms) | 5.101 | 4.900 | -3.9% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 7 | 7 | +0.0% |
| search-signal: p50 (ms) | 0.328 | 0.317 | -3.4% |
| search-signal: p95 (ms) | 0.514 | 0.509 | -1.0% |
| search-signal: peak RSS (MiB) | 5.93 | 6.23 | +5.1% |
| find-signal: CPU / 12 probes (ms) | 5.088 | 5.250 | +3.2% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 15,736 | 15,736 | +0.0% |
| find-signal: p50 (ms) | 0.332 | 0.346 | +4.2% |
| find-signal: p95 (ms) | 0.509 | 0.565 | +11.0% |
| find-signal: peak RSS (MiB) | 5.93 | 6.29 | +6.1% |
| point: CPU / 12 probes (ms) | 5.091 | 4.886 | -4.0% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.332 | 0.318 | -4.2% |
| point: p95 (ms) | 0.512 | 0.498 | -2.7% |
| point: peak RSS (MiB) | 5.93 | 6.23 | +5.1% |
| Write producer CPU time (ms) | — | 64.24 | — |

### requests

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 30.09 | 47.48 | +57.8% |
| Write throughput (Mobs/s) | 6.838 | 4.333 | -36.6% |
| File size (MiB) | 2.292 | 1.616 | -29.5% |
| Bytes per observation | 11.68 | 8.23 | -29.5% |
| Logical bytes / file bytes | 5.82 | 8.26 | +41.9% |
| Write peak RSS (MiB) | 9.77 | 12.03 | +23.1% |
| Capture time (ms) | 28.97 | 46.67 | +61.1% |
| Capture time (ns/tick) | 241.5 | 388.9 | +61.0% |
| File bytes per tick | 20.0 | 14.1 | -29.5% |
| Close time (ms) | 1.02 | 0.59 | -42.2% |
| Write CPU time (ms) | 31.05 | 48.44 | +56.0% |
| Write min / max (ms) | 29.33 / 30.29 | 47.05 / 49.05 | — |
| Read time (ms) | 16.53 | 13.28 | -19.7% |
| Read throughput (Mobs/s) | 12.445 | 15.492 | +24.5% |
| Read CPU time (ms) | 17.51 | 14.23 | -18.7% |
| Read peak RSS (MiB) | 7.77 | 6.62 | -14.8% |
| search-transaction: CPU / 12 probes (ms) | 24.683 | 3.944 | -84.0% |
| search-transaction: probes | 84 | 84 | +0.0% |
| search-transaction: matches | 63 | 63 | +0.0% |
| search-transaction: p50 (ms) | 1.904 | 0.204 | -89.3% |
| search-transaction: p95 (ms) | 2.412 | 0.415 | -82.8% |
| search-transaction: peak RSS (MiB) | 6.21 | 6.45 | +3.9% |
| find-transaction: CPU / 12 probes (ms) | 129.886 | 123.336 | -5.0% |
| find-transaction: probes | 84 | 84 | +0.0% |
| find-transaction: matches | 6,335 | 6,335 | +0.0% |
| find-transaction: p50 (ms) | 10.614 | 10.184 | -4.1% |
| find-transaction: p95 (ms) | 11.771 | 10.678 | -9.3% |
| find-transaction: peak RSS (MiB) | 7.79 | 6.67 | -14.4% |
| Write producer CPU time (ms) | — | 48.44 | — |

### many-generators

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 55.96 | 84.45 | +50.9% |
| Write throughput (Mobs/s) | 5.882 | 3.898 | -33.7% |
| File size (MiB) | 3.775 | 2.569 | -31.9% |
| Bytes per observation | 12.02 | 8.18 | -31.9% |
| Logical bytes / file bytes | 5.66 | 8.31 | +46.8% |
| Write peak RSS (MiB) | 9.37 | 12.01 | +28.2% |
| Capture time (ms) | 54.58 | 83.39 | +52.8% |
| Capture time (ns/tick) | 2274.1 | 3474.4 | +52.8% |
| File bytes per tick | 164.9 | 112.2 | -32.0% |
| Close time (ms) | 1.29 | 0.88 | -31.8% |
| Write CPU time (ms) | 56.91 | 85.39 | +50.0% |
| Write min / max (ms) | 55.23 / 56.76 | 84.06 / 85.56 | — |
| Read time (ms) | 29.34 | 21.35 | -27.2% |
| Read throughput (Mobs/s) | 11.218 | 15.415 | +37.4% |
| Read CPU time (ms) | 30.19 | 22.30 | -26.1% |
| Read peak RSS (MiB) | 9.28 | 7.67 | -17.3% |
| search-transaction: CPU / 12 probes (ms) | 26.424 | 4.915 | -81.4% |
| search-transaction: probes | 84 | 84 | +0.0% |
| search-transaction: matches | 35 | 35 | +0.0% |
| search-transaction: p50 (ms) | 1.924 | 0.373 | -80.6% |
| search-transaction: p95 (ms) | 3.760 | 0.430 | -88.6% |
| search-transaction: peak RSS (MiB) | 6.84 | 6.26 | -8.5% |
| find-transaction: CPU / 12 probes (ms) | 203.509 | 192.798 | -5.3% |
| find-transaction: probes | 84 | 84 | +0.0% |
| find-transaction: matches | 1,253 | 1,253 | +0.0% |
| find-transaction: p50 (ms) | 16.795 | 15.914 | -5.2% |
| find-transaction: p95 (ms) | 17.871 | 16.337 | -8.6% |
| find-transaction: peak RSS (MiB) | 9.27 | 7.64 | -17.6% |
| Write producer CPU time (ms) | — | 85.39 | — |

### mixed

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 36.00 | 49.57 | +37.7% |
| Write throughput (Mobs/s) | 37.258 | 27.060 | -27.4% |
| File size (MiB) | 2.500 | 2.192 | -12.3% |
| Bytes per observation | 1.95 | 1.71 | -12.3% |
| Logical bytes / file bytes | 9.44 | 10.77 | +14.1% |
| Write peak RSS (MiB) | 17.29 | 20.00 | +15.7% |
| Capture time (ms) | 31.19 | 45.23 | +45.0% |
| Capture time (ns/tick) | 1299.5 | 1884.7 | +45.0% |
| File bytes per tick | 109.2 | 95.8 | -12.3% |
| Close time (ms) | 4.67 | 4.07 | -12.8% |
| Write CPU time (ms) | 36.88 | 50.51 | +37.0% |
| Write min / max (ms) | 35.82 / 37.11 | 49.22 / 50.23 | — |
| Read time (ms) | 14.95 | 12.41 | -17.0% |
| Read throughput (Mobs/s) | 89.711 | 108.061 | +20.5% |
| Read CPU time (ms) | 15.93 | 13.34 | -16.3% |
| Read peak RSS (MiB) | 8.25 | 7.37 | -10.7% |
| search-signal: CPU / 12 probes (ms) | 1.786 | 1.888 | +5.7% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 686 | 686 | +0.0% |
| search-signal: p50 (ms) | 0.055 | 0.065 | +18.2% |
| search-signal: p95 (ms) | 0.192 | 0.211 | +9.9% |
| search-signal: peak RSS (MiB) | 6.08 | 6.78 | +11.5% |
| find-signal: CPU / 12 probes (ms) | 2.098 | 2.274 | +8.4% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 124,824 | 124,824 | +0.0% |
| find-signal: p50 (ms) | 0.066 | 0.080 | +21.2% |
| find-signal: p95 (ms) | 0.336 | 0.352 | +4.8% |
| find-signal: peak RSS (MiB) | 6.08 | 6.74 | +10.9% |
| point: CPU / 12 probes (ms) | 1.688 | 1.888 | +11.8% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.058 | 0.069 | +19.0% |
| point: p95 (ms) | 0.191 | 0.218 | +14.1% |
| point: peak RSS (MiB) | 6.08 | 6.77 | +11.3% |
| search-transaction: CPU / 12 probes (ms) | 25.294 | 3.788 | -85.0% |
| search-transaction: probes | 84 | 84 | +0.0% |
| search-transaction: matches | 56 | 56 | +0.0% |
| search-transaction: p50 (ms) | 2.048 | 0.218 | -89.4% |
| search-transaction: p95 (ms) | 2.694 | 0.396 | -85.3% |
| search-transaction: peak RSS (MiB) | 6.62 | 5.58 | -15.7% |
| find-transaction: CPU / 12 probes (ms) | 56.699 | 49.134 | -13.3% |
| find-transaction: probes | 84 | 84 | +0.0% |
| find-transaction: matches | 1,267 | 1,267 | +0.0% |
| find-transaction: p50 (ms) | 4.519 | 4.014 | -11.2% |
| find-transaction: p95 (ms) | 5.685 | 4.164 | -26.8% |
| find-transaction: peak RSS (MiB) | 6.61 | 5.77 | -12.7% |
| Write producer CPU time (ms) | — | 50.51 | — |

## Appendix B: complete common-format default suite

### Waveform results

#### core

Observations: 6,291,079. Null producer: 48.22 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 124.56 | 692.39 | 168.18 |
| Write throughput (Mobs/s) | 50.505 | 9.086 | 37.407 |
| Read time (ms) | 37.16 | 171.49 | 181.60 |
| Read throughput (Mobs/s) | 169.308 | 36.684 | 34.642 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 18.89 | 140.91 | 5.43 |
| Read peak RSS (MiB) | 13.83 | 26.23 | 5.15 |
| Capture time (ms) | 122.72 | 573.95 | 167.58 |
| Capture time (ns/tick) | 255.7 | 1195.7 | 349.1 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.61 | 117.51 | 0.12 |
| Write CPU time (ms) | 125.28 | 693.38 | 169.06 |
| Write min / max (ms) | 123.55 / 125.91 | 691.61 / 698.55 | 167.18 / 170.63 |
| Write producer CPU time (ms) | 125.28 | 693.38 | 169.06 |
| Read CPU time (ms) | 38.10 | 172.42 | 182.56 |
| search-signal: CPU / 12 probes (ms) | 2.985 | 35.557 | 610.975 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.114 | 2.518 | 55.612 |
| search-signal: p95 (ms) | 0.456 | 6.336 | 89.459 |
| search-signal: peak RSS (MiB) | 7.57 | 26.96 | 6.60 |
| find-signal: CPU / 12 probes (ms) | 14.424 | 49.056 | 1127.258 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.628 | 3.101 | 93.898 |
| find-signal: p95 (ms) | 5.148 | 8.633 | 97.425 |
| find-signal: peak RSS (MiB) | 10.04 | 26.91 | 6.57 |
| point: CPU / 12 probes (ms) | 3.812 | 7.292 | 620.825 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.121 | 0.360 | 57.187 |
| point: p95 (ms) | 0.765 | 1.532 | 91.326 |
| point: peak RSS (MiB) | 7.52 | 8.86 | 6.59 |

#### soc

Observations: 20,143,793. Null producer: 141.07 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 456.95 | 840.88 | 516.71 |
| Write throughput (Mobs/s) | 44.083 | 23.956 | 38.985 |
| Read time (ms) | 111.52 | 593.60 | 562.18 |
| Read throughput (Mobs/s) | 180.631 | 33.935 | 35.831 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 16.09 | 145.49 | 5.59 |
| Read peak RSS (MiB) | 30.08 | 22.61 | 5.30 |
| Capture time (ms) | 447.80 | 815.93 | 515.73 |
| Capture time (ns/tick) | 18658.2 | 33997.1 | 21488.8 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 7.30 | 24.32 | 0.17 |
| Write CPU time (ms) | 457.98 | 841.71 | 517.72 |
| Write min / max (ms) | 455.27 / 457.00 | 840.67 / 843.18 | 516.65 / 520.50 |
| Write producer CPU time (ms) | 457.98 | 841.71 | 517.72 |
| Read CPU time (ms) | 112.46 | 594.53 | 563.11 |
| search-signal: CPU / 12 probes (ms) | 4.016 | 11.645 | 1807.669 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.229 | 0.429 | 181.474 |
| search-signal: p95 (ms) | 0.486 | 6.263 | 250.847 |
| search-signal: peak RSS (MiB) | 6.49 | 22.64 | 6.60 |
| find-signal: CPU / 12 probes (ms) | 6.072 | 78.163 | 3263.267 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.390 | 6.810 | 271.088 |
| find-signal: p95 (ms) | 0.689 | 7.112 | 277.319 |
| find-signal: peak RSS (MiB) | 8.89 | 22.29 | 6.61 |
| point: CPU / 12 probes (ms) | 4.089 | 3.341 | 1776.286 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.230 | 0.198 | 178.754 |
| point: p95 (ms) | 0.494 | 0.315 | 247.377 |
| point: peak RSS (MiB) | 6.55 | 5.04 | 6.59 |

#### idle

Observations: 2,547,072. Null producer: 17.97 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 63.04 | 115.37 | 67.06 |
| Write throughput (Mobs/s) | 40.401 | 22.078 | 37.982 |
| Read time (ms) | 15.81 | 74.96 | 71.89 |
| Read throughput (Mobs/s) | 161.094 | 33.980 | 35.429 |
| File size (MiB) | 1.733 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.27 | 77.55 | 5.61 |
| Read peak RSS (MiB) | 7.14 | 14.68 | 5.22 |
| Capture time (ms) | 53.27 | 59.21 | 66.15 |
| Capture time (ns/tick) | 1109.9 | 1233.5 | 1378.2 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 8.45 | 55.72 | 0.16 |
| Write CPU time (ms) | 63.92 | 116.24 | 67.93 |
| Write min / max (ms) | 62.64 / 63.43 | 115.05 / 116.41 | 66.99 / 67.21 |
| Write producer CPU time (ms) | 63.92 | 116.24 | 67.93 |
| Read CPU time (ms) | 16.78 | 75.88 | 72.86 |
| search-signal: CPU / 12 probes (ms) | 4.980 | 9.358 | 189.570 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.323 | 0.332 | 13.853 |
| search-signal: p95 (ms) | 0.511 | 4.240 | 33.461 |
| search-signal: peak RSS (MiB) | 6.13 | 15.03 | 6.61 |
| find-signal: CPU / 12 probes (ms) | 5.253 | 9.216 | 423.265 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.344 | 0.333 | 35.154 |
| find-signal: p95 (ms) | 0.563 | 4.183 | 35.981 |
| find-signal: peak RSS (MiB) | 6.11 | 15.02 | 6.61 |
| point: CPU / 12 probes (ms) | 4.900 | 2.441 | 190.531 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.319 | 0.096 | 13.558 |
| point: p95 (ms) | 0.497 | 0.323 | 33.701 |
| point: peak RSS (MiB) | 6.12 | 5.45 | 6.59 |

### Transaction results

#### requests

Observations: 205,728. Null producer: 7.27 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 47.65 | 39.49 |
| Write throughput (Mobs/s) | 4.317 | 5.210 |
| Read time (ms) | 13.20 | 11.59 |
| Read throughput (Mobs/s) | 15.590 | 17.744 |
| File size (MiB) | 1.616 | 4.424 |
| Bytes per observation | 8.23 | 22.55 |
| Logical bytes / file bytes | 8.26 | 3.02 |
| Write peak RSS (MiB) | 11.90 | 4.86 |
| Read peak RSS (MiB) | 6.57 | 8.78 |
| Capture time (ms) | 46.85 | 39.26 |
| Capture time (ns/tick) | 390.4 | 327.2 |
| File bytes per tick | 14.1 | 38.7 |
| Close time (ms) | 0.59 | 0.18 |
| Write CPU time (ms) | 48.65 | 40.35 |
| Write min / max (ms) | 47.13 / 47.96 | 39.39 / 39.74 |
| Write producer CPU time (ms) | 48.65 | 40.35 |
| Read CPU time (ms) | 14.17 | 12.53 |
| search-transaction: CPU / 12 probes (ms) | 3.899 | 37.254 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.202 | 3.568 |
| search-transaction: p95 (ms) | 0.412 | 4.551 |
| search-transaction: peak RSS (MiB) | 6.42 | 8.77 |
| find-transaction: CPU / 12 probes (ms) | 123.157 | 61.757 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 10.133 | 5.085 |
| find-transaction: p95 (ms) | 10.713 | 5.336 |
| find-transaction: peak RSS (MiB) | 6.57 | 8.77 |

#### many-generators

Observations: 329,168. Null producer: 13.81 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 83.69 | 67.24 |
| Write throughput (Mobs/s) | 3.933 | 4.895 |
| Read time (ms) | 21.75 | 21.10 |
| Read throughput (Mobs/s) | 15.137 | 15.601 |
| File size (MiB) | 2.569 | 7.571 |
| Bytes per observation | 8.18 | 24.12 |
| Logical bytes / file bytes | 8.31 | 2.82 |
| Write peak RSS (MiB) | 11.95 | 6.04 |
| Read peak RSS (MiB) | 7.54 | 11.82 |
| Capture time (ms) | 82.59 | 66.49 |
| Capture time (ns/tick) | 3441.4 | 2770.5 |
| File bytes per tick | 112.2 | 330.8 |
| Close time (ms) | 0.88 | 0.70 |
| Write CPU time (ms) | 84.65 | 68.17 |
| Write min / max (ms) | 83.66 / 85.56 | 66.92 / 67.68 |
| Write producer CPU time (ms) | 84.65 | 68.17 |
| Read CPU time (ms) | 22.73 | 22.07 |
| search-transaction: CPU / 12 probes (ms) | 4.943 | 10.072 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.361 | 0.616 |
| search-transaction: p95 (ms) | 0.438 | 1.402 |
| search-transaction: peak RSS (MiB) | 6.20 | 11.86 |
| find-transaction: CPU / 12 probes (ms) | 192.589 | 17.583 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 15.896 | 1.363 |
| find-transaction: p95 (ms) | 16.482 | 1.545 |
| find-transaction: peak RSS (MiB) | 7.62 | 11.80 |

### Mixed signal and transaction results

#### mixed

Observations: 1,341,285. Null producer: 11.99 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 50.09 | 94.45 | 50.75 |
| Write throughput (Mobs/s) | 26.775 | 14.200 | 26.427 |
| Read time (ms) | 12.36 | 39.05 | 41.22 |
| Read throughput (Mobs/s) | 108.550 | 34.344 | 32.543 |
| File size (MiB) | 2.192 | 4.010 | 31.812 |
| Bytes per observation | 1.71 | 3.13 | 24.87 |
| Logical bytes / file bytes | 10.77 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 19.77 | 40.99 | 6.06 |
| Read peak RSS (MiB) | 7.15 | 9.33 | 6.13 |
| Capture time (ms) | 45.78 | 46.48 | 49.92 |
| Capture time (ns/tick) | 1907.3 | 1936.8 | 2080.0 |
| File bytes per tick | 95.8 | 175.2 | 1389.9 |
| Close time (ms) | 4.04 | 48.64 | 0.27 |
| Write CPU time (ms) | 51.03 | 95.29 | 51.72 |
| Write min / max (ms) | 49.50 / 51.79 | 94.17 / 95.44 | 50.63 / 50.85 |
| Write producer CPU time (ms) | 51.03 | 95.29 | 51.72 |
| Read CPU time (ms) | 13.30 | 40.01 | 42.19 |
| search-signal: CPU / 12 probes (ms) | 1.914 | 5.851 | 121.349 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.066 | 0.236 | 11.998 |
| search-signal: p95 (ms) | 0.219 | 2.067 | 17.089 |
| search-signal: peak RSS (MiB) | 6.69 | 9.82 | 6.59 |
| find-signal: CPU / 12 probes (ms) | 2.288 | 5.802 | 218.117 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.075 | 0.236 | 18.008 |
| find-signal: p95 (ms) | 0.357 | 2.016 | 18.705 |
| find-signal: peak RSS (MiB) | 6.70 | 9.79 | 6.59 |
| point: CPU / 12 probes (ms) | 1.850 | 1.570 | 120.241 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.069 | 0.038 | 11.837 |
| point: p95 (ms) | 0.223 | 0.168 | 16.839 |
| point: peak RSS (MiB) | 6.68 | 5.15 | 6.62 |
| search-transaction: CPU / 12 probes (ms) | 3.760 | 7.670 | 7.838 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.213 | 0.434 | 0.430 |
| search-transaction: p95 (ms) | 0.393 | 1.113 | 1.115 |
| search-transaction: peak RSS (MiB) | 5.44 | 6.01 | 6.02 |
| find-transaction: CPU / 12 probes (ms) | 49.167 | 14.070 | 14.051 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 4.012 | 1.099 | 1.094 |
| find-transaction: p95 (ms) | 4.187 | 1.177 | 1.214 |
| find-transaction: peak RSS (MiB) | 5.71 | 6.02 | 6.06 |

### Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. VTR uses its default reader mode without whole-section CRC scans; validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.

## Appendix C: workers sharing one CPU

### core

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 123.55 | 122.95 |
| Write throughput (Mobs/s) | 50.920 | 51.167 |
| File size (MiB) | 7.695 | 7.695 |
| Bytes per observation | 1.28 | 1.28 |
| Logical bytes / file bytes | 11.86 | 11.86 |
| Write peak RSS (MiB) | 18.95 | 24.21 |
| Capture time (ms) | 121.68 | 121.39 |
| Capture time (ns/tick) | 253.5 | 252.9 |
| File bytes per tick | 16.8 | 16.8 |
| Close time (ms) | 1.61 | 1.33 |
| Write CPU time (ms) | 124.47 | 123.81 |
| Write min / max (ms) | 123.19 / 124.05 | 120.57 / 123.42 |
| Write producer CPU time (ms) | 124.47 | 102.36 |
| Read time (ms) | 36.90 | 36.93 |
| Read throughput (Mobs/s) | 170.495 | 170.357 |
| Read CPU time (ms) | 37.86 | 37.89 |
| Read peak RSS (MiB) | 14.02 | 13.99 |
| search-signal: CPU / 12 probes (ms) | 3.012 | 3.037 |
| search-signal: probes | 60 | 60 |
| search-signal: matches | 485 | 485 |
| search-signal: p50 (ms) | 0.111 | 0.114 |
| search-signal: p95 (ms) | 0.448 | 0.451 |
| search-signal: peak RSS (MiB) | 7.68 | 7.61 |
| find-signal: CPU / 12 probes (ms) | 14.303 | 14.282 |
| find-signal: probes | 60 | 60 |
| find-signal: matches | 1,781,045 | 1,781,045 |
| find-signal: p50 (ms) | 0.636 | 0.623 |
| find-signal: p95 (ms) | 5.119 | 5.073 |
| find-signal: peak RSS (MiB) | 10.18 | 10.17 |
| point: CPU / 12 probes (ms) | 3.875 | 3.839 |
| point: probes | 60 | 60 |
| point: matches | 60 | 60 |
| point: p50 (ms) | 0.125 | 0.119 |
| point: p95 (ms) | 0.764 | 0.764 |
| point: peak RSS (MiB) | 7.68 | 7.73 |
| accepted_jobs | 0 | 10 |
| written_jobs | 0 | 10 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 151121636 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 0 |
| wait_nanoseconds | 0 | 0 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 16777223 | 16777223 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 0 | 0 |
| payload_pool_bytes | 0 | 0 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 0 |

### soc

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 455.64 | 511.79 |
| Write throughput (Mobs/s) | 44.210 | 39.360 |
| File size (MiB) | 24.579 | 24.579 |
| Bytes per observation | 1.28 | 1.28 |
| Logical bytes / file bytes | 11.89 | 11.89 |
| Write peak RSS (MiB) | 16.20 | 24.27 |
| Capture time (ms) | 447.06 | 502.67 |
| Capture time (ns/tick) | 18627.7 | 20944.5 |
| File bytes per tick | 1073.9 | 1073.9 |
| Close time (ms) | 7.28 | 7.56 |
| Write CPU time (ms) | 456.54 | 512.71 |
| Write min / max (ms) | 454.72 / 461.00 | 507.65 / 514.00 |
| Write producer CPU time (ms) | 456.54 | 372.54 |
| Read time (ms) | 110.66 | 109.72 |
| Read throughput (Mobs/s) | 182.037 | 183.587 |
| Read CPU time (ms) | 111.60 | 110.65 |
| Read peak RSS (MiB) | 30.18 | 30.22 |
| search-signal: CPU / 12 probes (ms) | 4.050 | 4.025 |
| search-signal: probes | 60 | 60 |
| search-signal: matches | 495 | 495 |
| search-signal: p50 (ms) | 0.231 | 0.230 |
| search-signal: p95 (ms) | 0.483 | 0.480 |
| search-signal: peak RSS (MiB) | 6.68 | 8.03 |
| find-signal: CPU / 12 probes (ms) | 6.008 | 6.128 |
| find-signal: probes | 60 | 60 |
| find-signal: matches | 89,195 | 89,195 |
| find-signal: p50 (ms) | 0.384 | 0.385 |
| find-signal: p95 (ms) | 0.691 | 0.684 |
| find-signal: peak RSS (MiB) | 9.02 | 22.89 |
| point: CPU / 12 probes (ms) | 4.003 | 4.049 |
| point: probes | 60 | 60 |
| point: matches | 60 | 60 |
| point: p50 (ms) | 0.230 | 0.230 |
| point: p95 (ms) | 0.485 | 0.486 |
| point: peak RSS (MiB) | 6.65 | 8.03 |
| accepted_jobs | 0 | 19 |
| written_jobs | 0 | 19 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 151662736 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 0 |
| wait_nanoseconds | 0 | 0 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 16777227 | 16777227 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 0 | 0 |
| payload_pool_bytes | 0 | 0 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 0 |

### idle

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 63.51 | 69.19 |
| Write throughput (Mobs/s) | 40.103 | 36.811 |
| File size (MiB) | 1.733 | 1.733 |
| Bytes per observation | 0.71 | 0.71 |
| Logical bytes / file bytes | 21.34 | 21.34 |
| Write peak RSS (MiB) | 15.43 | 20.44 |
| Capture time (ms) | 53.89 | 58.70 |
| Capture time (ns/tick) | 1122.6 | 1222.9 |
| File bytes per tick | 37.8 | 37.8 |
| Close time (ms) | 8.29 | 9.00 |
| Write CPU time (ms) | 64.37 | 70.03 |
| Write min / max (ms) | 62.79 / 63.84 | 68.82 / 70.16 |
| Write producer CPU time (ms) | 64.37 | 51.54 |
| Read time (ms) | 15.37 | 15.60 |
| Read throughput (Mobs/s) | 165.701 | 163.282 |
| Read CPU time (ms) | 16.33 | 16.55 |
| Read peak RSS (MiB) | 7.33 | 7.26 |
| search-signal: CPU / 12 probes (ms) | 4.955 | 4.957 |
| search-signal: probes | 60 | 60 |
| search-signal: matches | 5 | 5 |
| search-signal: p50 (ms) | 0.321 | 0.321 |
| search-signal: p95 (ms) | 0.510 | 0.512 |
| search-signal: peak RSS (MiB) | 6.17 | 6.80 |
| find-signal: CPU / 12 probes (ms) | 5.321 | 5.276 |
| find-signal: probes | 60 | 60 |
| find-signal: matches | 11,240 | 11,240 |
| find-signal: p50 (ms) | 0.347 | 0.344 |
| find-signal: p95 (ms) | 0.572 | 0.566 |
| find-signal: peak RSS (MiB) | 6.25 | 7.21 |
| point: CPU / 12 probes (ms) | 4.916 | 4.956 |
| point: probes | 60 | 60 |
| point: matches | 60 | 60 |
| point: p50 (ms) | 0.318 | 0.322 |
| point: p95 (ms) | 0.502 | 0.516 |
| point: peak RSS (MiB) | 6.29 | 6.85 |
| accepted_jobs | 0 | 7 |
| written_jobs | 0 | 7 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 155618512 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 0 |
| wait_nanoseconds | 0 | 0 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 16777221 | 16777221 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 0 | 0 |
| payload_pool_bytes | 0 | 0 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 0 |

### requests

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 47.71 | 56.35 |
| Write throughput (Mobs/s) | 4.312 | 3.651 |
| File size (MiB) | 1.616 | 1.616 |
| Bytes per observation | 8.23 | 8.23 |
| Logical bytes / file bytes | 8.26 | 8.26 |
| Write peak RSS (MiB) | 12.02 | 21.66 |
| Capture time (ms) | 46.92 | 55.10 |
| Capture time (ns/tick) | 391.0 | 459.2 |
| File bytes per tick | 14.1 | 14.1 |
| Close time (ms) | 0.59 | 1.03 |
| Write CPU time (ms) | 48.65 | 57.17 |
| Write min / max (ms) | 46.98 / 48.57 | 55.93 / 60.10 |
| Write producer CPU time (ms) | 48.65 | 29.63 |
| Read time (ms) | 13.43 | 13.11 |
| Read throughput (Mobs/s) | 15.319 | 15.688 |
| Read CPU time (ms) | 14.40 | 14.07 |
| Read peak RSS (MiB) | 6.67 | 6.59 |
| search-transaction: CPU / 12 probes (ms) | 3.858 | 3.893 |
| search-transaction: probes | 60 | 60 |
| search-transaction: matches | 45 | 45 |
| search-transaction: p50 (ms) | 0.203 | 0.204 |
| search-transaction: p95 (ms) | 0.402 | 0.409 |
| search-transaction: peak RSS (MiB) | 6.45 | 6.45 |
| find-transaction: CPU / 12 probes (ms) | 124.313 | 124.483 |
| find-transaction: probes | 60 | 60 |
| find-transaction: matches | 4,525 | 4,525 |
| find-transaction: p50 (ms) | 10.214 | 10.212 |
| find-transaction: p95 (ms) | 13.988 | 10.825 |
| find-transaction: peak RSS (MiB) | 6.60 | 6.64 |
| accepted_jobs | 0 | 16 |
| written_jobs | 0 | 16 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 260944095 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 1 |
| wait_nanoseconds | 0 | 95753 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 0 | 0 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 4194320 | 4194320 |
| payload_pool_bytes | 0 | 8368448 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 185563 |

### many-generators

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 84.11 | 98.14 |
| Write throughput (Mobs/s) | 3.913 | 3.354 |
| File size (MiB) | 2.569 | 2.569 |
| Bytes per observation | 8.18 | 8.18 |
| Logical bytes / file bytes | 8.31 | 8.31 |
| Write peak RSS (MiB) | 12.12 | 13.52 |
| Capture time (ms) | 83.04 | 96.50 |
| Capture time (ns/tick) | 3459.9 | 4020.6 |
| File bytes per tick | 112.2 | 112.2 |
| Close time (ms) | 0.87 | 1.38 |
| Write CPU time (ms) | 85.05 | 99.04 |
| Write min / max (ms) | 83.54 / 86.96 | 96.61 / 98.81 |
| Write producer CPU time (ms) | 85.05 | 53.19 |
| Read time (ms) | 21.21 | 21.68 |
| Read throughput (Mobs/s) | 15.523 | 15.184 |
| Read CPU time (ms) | 22.17 | 22.70 |
| Read peak RSS (MiB) | 7.65 | 7.64 |
| search-transaction: CPU / 12 probes (ms) | 4.907 | 4.907 |
| search-transaction: probes | 60 | 60 |
| search-transaction: matches | 25 | 25 |
| search-transaction: p50 (ms) | 0.375 | 0.392 |
| search-transaction: p95 (ms) | 0.447 | 0.422 |
| search-transaction: peak RSS (MiB) | 6.26 | 6.29 |
| find-transaction: CPU / 12 probes (ms) | 191.397 | 193.160 |
| find-transaction: probes | 60 | 60 |
| find-transaction: matches | 895 | 895 |
| find-transaction: p50 (ms) | 15.857 | 15.947 |
| find-transaction: p95 (ms) | 16.317 | 16.406 |
| find-transaction: peak RSS (MiB) | 7.64 | 7.63 |
| accepted_jobs | 0 | 22 |
| written_jobs | 0 | 22 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 260951263 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 0 |
| wait_nanoseconds | 0 | 0 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 0 | 0 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 4194320 | 4194320 |
| payload_pool_bytes | 0 | 8368448 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 309003 |

### mixed

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 49.57 | 53.95 |
| Write throughput (Mobs/s) | 27.058 | 24.863 |
| File size (MiB) | 2.192 | 2.192 |
| Bytes per observation | 1.71 | 1.71 |
| Logical bytes / file bytes | 10.77 | 10.77 |
| Write peak RSS (MiB) | 19.86 | 26.66 |
| Capture time (ms) | 45.25 | 48.49 |
| Capture time (ns/tick) | 1885.3 | 2020.4 |
| File bytes per tick | 95.8 | 95.8 |
| Close time (ms) | 4.06 | 5.13 |
| Write CPU time (ms) | 50.45 | 54.85 |
| Write min / max (ms) | 49.28 / 50.24 | 53.56 / 59.06 |
| Write producer CPU time (ms) | 50.45 | 38.13 |
| Read time (ms) | 12.30 | 12.34 |
| Read throughput (Mobs/s) | 109.024 | 108.735 |
| Read CPU time (ms) | 13.25 | 13.29 |
| Read peak RSS (MiB) | 7.43 | 7.39 |
| search-signal: CPU / 12 probes (ms) | 1.882 | 1.891 |
| search-signal: probes | 60 | 60 |
| search-signal: matches | 490 | 490 |
| search-signal: p50 (ms) | 0.064 | 0.063 |
| search-signal: p95 (ms) | 0.208 | 0.213 |
| search-signal: peak RSS (MiB) | 6.78 | 6.70 |
| find-signal: CPU / 12 probes (ms) | 2.256 | 2.285 |
| find-signal: probes | 60 | 60 |
| find-signal: matches | 89,160 | 89,160 |
| find-signal: p50 (ms) | 0.077 | 0.075 |
| find-signal: p95 (ms) | 0.352 | 0.347 |
| find-signal: peak RSS (MiB) | 6.78 | 6.71 |
| point: CPU / 12 probes (ms) | 1.802 | 1.805 |
| point: probes | 60 | 60 |
| point: matches | 60 | 60 |
| point: p50 (ms) | 0.068 | 0.068 |
| point: p95 (ms) | 0.212 | 0.212 |
| point: peak RSS (MiB) | 6.78 | 6.73 |
| search-transaction: CPU / 12 probes (ms) | 3.787 | 3.746 |
| search-transaction: probes | 60 | 60 |
| search-transaction: matches | 40 | 40 |
| search-transaction: p50 (ms) | 0.221 | 0.213 |
| search-transaction: p95 (ms) | 0.397 | 0.394 |
| search-transaction: peak RSS (MiB) | 5.56 | 5.59 |
| find-transaction: CPU / 12 probes (ms) | 49.548 | 49.281 |
| find-transaction: probes | 60 | 60 |
| find-transaction: matches | 905 | 905 |
| find-transaction: p50 (ms) | 4.020 | 4.008 |
| find-transaction: p95 (ms) | 4.282 | 4.281 |
| find-transaction: peak RSS (MiB) | 5.73 | 5.79 |
| accepted_jobs | 0 | 11 |
| written_jobs | 0 | 11 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 260945119 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 0 |
| wait_nanoseconds | 0 | 0 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 14309357 | 14309357 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 4194320 | 4194320 |
| payload_pool_bytes | 0 | 8368448 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 62155 |


## Appendix D: workers sharing three CPUs

### core

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 124.04 | 108.30 |
| Write throughput (Mobs/s) | 50.718 | 58.089 |
| File size (MiB) | 7.695 | 7.695 |
| Bytes per observation | 1.28 | 1.28 |
| Logical bytes / file bytes | 11.86 | 11.86 |
| Write peak RSS (MiB) | 19.00 | 25.50 |
| Capture time (ms) | 122.23 | 106.30 |
| Capture time (ns/tick) | 254.6 | 221.5 |
| File bytes per tick | 16.8 | 16.8 |
| Close time (ms) | 1.60 | 1.82 |
| Write CPU time (ms) | 124.98 | 122.95 |
| Write min / max (ms) | 123.34 / 128.64 | 107.83 / 108.75 |
| Write producer CPU time (ms) | 124.98 | 101.29 |
| Read time (ms) | 36.86 | 37.43 |
| Read throughput (Mobs/s) | 170.660 | 168.089 |
| Read CPU time (ms) | 37.86 | 38.46 |
| Read peak RSS (MiB) | 13.98 | 13.98 |
| search-signal: CPU / 12 probes (ms) | 3.057 | 3.092 |
| search-signal: probes | 60 | 60 |
| search-signal: matches | 485 | 485 |
| search-signal: p50 (ms) | 0.113 | 0.113 |
| search-signal: p95 (ms) | 0.441 | 0.440 |
| search-signal: peak RSS (MiB) | 7.67 | 7.60 |
| find-signal: CPU / 12 probes (ms) | 14.344 | 14.338 |
| find-signal: probes | 60 | 60 |
| find-signal: matches | 1,781,045 | 1,781,045 |
| find-signal: p50 (ms) | 0.645 | 0.629 |
| find-signal: p95 (ms) | 5.112 | 5.100 |
| find-signal: peak RSS (MiB) | 10.16 | 10.16 |
| point: CPU / 12 probes (ms) | 3.780 | 3.910 |
| point: probes | 60 | 60 |
| point: matches | 60 | 60 |
| point: p50 (ms) | 0.121 | 0.120 |
| point: p95 (ms) | 0.766 | 0.772 |
| point: peak RSS (MiB) | 7.71 | 7.69 |
| accepted_jobs | 0 | 10 |
| written_jobs | 0 | 10 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 151121636 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 0 |
| wait_nanoseconds | 0 | 0 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 16777223 | 16777223 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 0 | 0 |
| payload_pool_bytes | 0 | 0 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 0 |

### soc

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 457.36 | 373.78 |
| Write throughput (Mobs/s) | 44.043 | 53.892 |
| File size (MiB) | 24.579 | 24.579 |
| Bytes per observation | 1.28 | 1.28 |
| Logical bytes / file bytes | 11.89 | 11.89 |
| Write peak RSS (MiB) | 16.17 | 24.78 |
| Capture time (ms) | 448.76 | 363.87 |
| Capture time (ns/tick) | 18698.2 | 15161.1 |
| File bytes per tick | 1073.9 | 1073.9 |
| Close time (ms) | 7.26 | 8.45 |
| Write CPU time (ms) | 458.27 | 507.67 |
| Write min / max (ms) | 456.33 / 460.78 | 371.74 / 380.81 |
| Write producer CPU time (ms) | 458.27 | 366.92 |
| Read time (ms) | 109.90 | 110.30 |
| Read throughput (Mobs/s) | 183.289 | 182.633 |
| Read CPU time (ms) | 111.00 | 111.32 |
| Read peak RSS (MiB) | 30.22 | 30.23 |
| search-signal: CPU / 12 probes (ms) | 4.058 | 4.116 |
| search-signal: probes | 60 | 60 |
| search-signal: matches | 495 | 495 |
| search-signal: p50 (ms) | 0.234 | 0.230 |
| search-signal: p95 (ms) | 0.471 | 0.468 |
| search-signal: peak RSS (MiB) | 6.63 | 8.02 |
| find-signal: CPU / 12 probes (ms) | 6.166 | 6.074 |
| find-signal: probes | 60 | 60 |
| find-signal: matches | 89,195 | 89,195 |
| find-signal: p50 (ms) | 0.392 | 0.383 |
| find-signal: p95 (ms) | 0.672 | 0.673 |
| find-signal: peak RSS (MiB) | 9.00 | 22.89 |
| point: CPU / 12 probes (ms) | 3.994 | 4.144 |
| point: probes | 60 | 60 |
| point: matches | 60 | 60 |
| point: p50 (ms) | 0.233 | 0.231 |
| point: p95 (ms) | 0.452 | 0.461 |
| point: peak RSS (MiB) | 6.68 | 8.04 |
| accepted_jobs | 0 | 19 |
| written_jobs | 0 | 19 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 151662736 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 0 |
| wait_nanoseconds | 0 | 0 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 16777227 | 16777227 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 0 | 0 |
| payload_pool_bytes | 0 | 0 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 0 |

### idle

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 63.04 | 59.64 |
| Write throughput (Mobs/s) | 40.401 | 42.705 |
| File size (MiB) | 1.733 | 1.733 |
| Bytes per observation | 0.71 | 0.71 |
| Logical bytes / file bytes | 21.34 | 21.34 |
| Write peak RSS (MiB) | 15.40 | 20.24 |
| Capture time (ms) | 53.33 | 48.74 |
| Capture time (ns/tick) | 1111.0 | 1015.4 |
| File bytes per tick | 37.8 | 37.8 |
| Close time (ms) | 8.40 | 9.47 |
| Write CPU time (ms) | 63.89 | 70.46 |
| Write min / max (ms) | 62.40 / 63.43 | 58.62 / 60.93 |
| Write producer CPU time (ms) | 63.89 | 51.51 |
| Read time (ms) | 15.48 | 15.45 |
| Read throughput (Mobs/s) | 164.521 | 164.871 |
| Read CPU time (ms) | 16.48 | 16.52 |
| Read peak RSS (MiB) | 7.31 | 7.24 |
| search-signal: CPU / 12 probes (ms) | 4.930 | 4.918 |
| search-signal: probes | 60 | 60 |
| search-signal: matches | 5 | 5 |
| search-signal: p50 (ms) | 0.320 | 0.321 |
| search-signal: p95 (ms) | 0.491 | 0.491 |
| search-signal: peak RSS (MiB) | 6.26 | 6.84 |
| find-signal: CPU / 12 probes (ms) | 5.336 | 5.271 |
| find-signal: probes | 60 | 60 |
| find-signal: matches | 11,240 | 11,240 |
| find-signal: p50 (ms) | 0.345 | 0.346 |
| find-signal: p95 (ms) | 0.535 | 0.536 |
| find-signal: peak RSS (MiB) | 6.29 | 7.16 |
| point: CPU / 12 probes (ms) | 4.933 | 4.828 |
| point: probes | 60 | 60 |
| point: matches | 60 | 60 |
| point: p50 (ms) | 0.319 | 0.320 |
| point: p95 (ms) | 0.477 | 0.465 |
| point: peak RSS (MiB) | 6.23 | 6.80 |
| accepted_jobs | 0 | 7 |
| written_jobs | 0 | 7 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 155618512 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 0 |
| wait_nanoseconds | 0 | 0 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 16777221 | 16777221 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 0 | 0 |
| payload_pool_bytes | 0 | 0 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 0 |

### requests

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 47.34 | 47.34 |
| Write throughput (Mobs/s) | 4.346 | 4.346 |
| File size (MiB) | 1.616 | 1.616 |
| Bytes per observation | 8.23 | 8.23 |
| Logical bytes / file bytes | 8.26 | 8.26 |
| Write peak RSS (MiB) | 11.94 | 22.92 |
| Capture time (ms) | 46.53 | 42.38 |
| Capture time (ns/tick) | 387.8 | 353.2 |
| File bytes per tick | 14.1 | 14.1 |
| Close time (ms) | 0.59 | 4.34 |
| Write CPU time (ms) | 48.24 | 65.74 |
| Write min / max (ms) | 47.05 / 48.20 | 42.04 / 52.02 |
| Write producer CPU time (ms) | 48.24 | 35.41 |
| Read time (ms) | 13.10 | 13.17 |
| Read throughput (Mobs/s) | 15.702 | 15.615 |
| Read CPU time (ms) | 14.18 | 14.14 |
| Read peak RSS (MiB) | 6.62 | 6.64 |
| search-transaction: CPU / 12 probes (ms) | 3.956 | 3.925 |
| search-transaction: probes | 60 | 60 |
| search-transaction: matches | 45 | 45 |
| search-transaction: p50 (ms) | 0.204 | 0.206 |
| search-transaction: p95 (ms) | 0.409 | 0.407 |
| search-transaction: peak RSS (MiB) | 6.49 | 6.49 |
| find-transaction: CPU / 12 probes (ms) | 123.054 | 123.619 |
| find-transaction: probes | 60 | 60 |
| find-transaction: matches | 4,525 | 4,525 |
| find-transaction: p50 (ms) | 10.151 | 10.256 |
| find-transaction: p95 (ms) | 10.377 | 10.569 |
| find-transaction: peak RSS (MiB) | 6.67 | 6.57 |
| accepted_jobs | 0 | 16 |
| written_jobs | 0 | 16 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 260944095 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 7 |
| wait_nanoseconds | 0 | 5674727 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 0 | 0 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 4194320 | 4194320 |
| payload_pool_bytes | 0 | 9659008 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 165398 |

### many-generators

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 84.65 | 66.94 |
| Write throughput (Mobs/s) | 3.889 | 4.917 |
| File size (MiB) | 2.569 | 2.569 |
| Bytes per observation | 8.18 | 8.18 |
| Logical bytes / file bytes | 8.31 | 8.31 |
| Write peak RSS (MiB) | 12.00 | 22.10 |
| Capture time (ms) | 83.54 | 63.09 |
| Capture time (ns/tick) | 3480.7 | 2628.8 |
| File bytes per tick | 112.2 | 112.2 |
| Close time (ms) | 0.88 | 4.06 |
| Write CPU time (ms) | 85.66 | 106.65 |
| Write min / max (ms) | 83.81 / 85.48 | 65.22 / 73.74 |
| Write producer CPU time (ms) | 85.66 | 57.74 |
| Read time (ms) | 21.41 | 21.68 |
| Read throughput (Mobs/s) | 15.377 | 15.186 |
| Read CPU time (ms) | 22.55 | 22.62 |
| Read peak RSS (MiB) | 7.65 | 7.61 |
| search-transaction: CPU / 12 probes (ms) | 4.897 | 5.023 |
| search-transaction: probes | 60 | 60 |
| search-transaction: matches | 25 | 25 |
| search-transaction: p50 (ms) | 0.361 | 0.370 |
| search-transaction: p95 (ms) | 0.417 | 0.430 |
| search-transaction: peak RSS (MiB) | 6.25 | 6.28 |
| find-transaction: CPU / 12 probes (ms) | 193.026 | 191.642 |
| find-transaction: probes | 60 | 60 |
| find-transaction: matches | 895 | 895 |
| find-transaction: p50 (ms) | 15.911 | 15.803 |
| find-transaction: p95 (ms) | 16.314 | 16.207 |
| find-transaction: peak RSS (MiB) | 7.62 | 7.72 |
| accepted_jobs | 0 | 22 |
| written_jobs | 0 | 22 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 260951263 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 6 |
| wait_nanoseconds | 0 | 3173203 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 0 | 0 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 4194320 | 4194320 |
| payload_pool_bytes | 0 | 9659008 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 288838 |

### mixed

| Metric | sync-16 | worker2-16 |
|---|---:|---:|
| Write time (ms) | 49.73 | 51.63 |
| Write throughput (Mobs/s) | 26.974 | 25.978 |
| File size (MiB) | 2.192 | 2.192 |
| Bytes per observation | 1.71 | 1.71 |
| Logical bytes / file bytes | 10.77 | 10.77 |
| Write peak RSS (MiB) | 19.89 | 26.85 |
| Capture time (ms) | 45.40 | 42.63 |
| Capture time (ns/tick) | 1891.5 | 1776.2 |
| File bytes per tick | 95.8 | 95.8 |
| Close time (ms) | 4.06 | 8.58 |
| Write CPU time (ms) | 50.69 | 58.39 |
| Write min / max (ms) | 49.34 / 50.77 | 51.40 / 52.09 |
| Write producer CPU time (ms) | 50.69 | 40.65 |
| Read time (ms) | 12.27 | 12.31 |
| Read throughput (Mobs/s) | 109.306 | 108.924 |
| Read CPU time (ms) | 13.29 | 13.35 |
| Read peak RSS (MiB) | 7.39 | 7.41 |
| search-signal: CPU / 12 probes (ms) | 1.810 | 1.942 |
| search-signal: probes | 60 | 60 |
| search-signal: matches | 490 | 490 |
| search-signal: p50 (ms) | 0.064 | 0.064 |
| search-signal: p95 (ms) | 0.192 | 0.204 |
| search-signal: peak RSS (MiB) | 6.75 | 6.71 |
| find-signal: CPU / 12 probes (ms) | 2.328 | 2.149 |
| find-signal: probes | 60 | 60 |
| find-signal: matches | 89,160 | 89,160 |
| find-signal: p50 (ms) | 0.078 | 0.077 |
| find-signal: p95 (ms) | 0.354 | 0.352 |
| find-signal: peak RSS (MiB) | 6.73 | 6.75 |
| point: CPU / 12 probes (ms) | 1.836 | 1.850 |
| point: probes | 60 | 60 |
| point: matches | 60 | 60 |
| point: p50 (ms) | 0.069 | 0.068 |
| point: p95 (ms) | 0.199 | 0.199 |
| point: peak RSS (MiB) | 6.76 | 6.72 |
| search-transaction: CPU / 12 probes (ms) | 3.813 | 3.657 |
| search-transaction: probes | 60 | 60 |
| search-transaction: matches | 40 | 40 |
| search-transaction: p50 (ms) | 0.217 | 0.220 |
| search-transaction: p95 (ms) | 0.395 | 0.397 |
| search-transaction: peak RSS (MiB) | 5.59 | 5.61 |
| find-transaction: CPU / 12 probes (ms) | 49.270 | 49.292 |
| find-transaction: probes | 60 | 60 |
| find-transaction: matches | 905 | 905 |
| find-transaction: p50 (ms) | 4.015 | 4.018 |
| find-transaction: p95 (ms) | 4.181 | 4.190 |
| find-transaction: peak RSS (MiB) | 5.78 | 5.73 |
| accepted_jobs | 0 | 11 |
| written_jobs | 0 | 11 |
| peak_jobs | 0 | 3 |
| peak_charged_bytes | 0 | 260945119 |
| peak_encoding | 0 | 1 |
| peak_ready | 0 | 1 |
| wait_count | 0 | 1 |
| wait_nanoseconds | 0 | 2735333 |
| oversized_jobs | 0 | 0 |
| peak_wave_buffer_bytes | 14309357 | 14309357 |
| peak_record_buffer_bytes | 0 | 0 |
| peak_transaction_buffer_bytes | 4194320 | 4194320 |
| payload_pool_bytes | 0 | 8893120 |
| payload_pool_limit | 0 | 16777216 |
| payload_reuses | 0 | 53957 |


## Appendix E: retained navigation results, one CPU

### balanced

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 15.206 | 27.419 | 37.468 |
| write CPU ms | 16.042 | 28.260 | 38.271 |
| write max peak RSS MiB | 9.414 | 15.680 | 26.652 |
| read wall ms | 11.432 | 7.374 | 7.455 |
| read CPU ms | 12.418 | 8.363 | 8.466 |
| read max peak RSS MiB | 5.926 | 5.531 | 5.559 |
| null wall ms | 0.430 | 0.430 | 0.447 |
| null CPU ms | 1.325 | 1.405 | 1.410 |
| null max peak RSS MiB | 4.648 | 4.719 | 4.738 |
| write capture ms | 14.412 | 26.114 | 31.879 |
| write close ms | 0.699 | 1.054 | 5.235 |
| write file MiB | 0.408 | 0.486 | 0.486 |
| cold track: p50 ms | 3.580 | 0.051 | 0.051 |
| cold track: p95 ms | 4.391 | 0.138 | 0.140 |
| cold track: peak RSS MiB | 6.059 | 5.547 | 5.547 |
| cold track: CPU ms/12 | 38.515 | 1.618 | 1.644 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 34.000 | 34.000 | 34.000 |
| cold labels: p50 ms | 5.851 | 1.531 | 1.607 |
| cold labels: p95 ms | 11.219 | 3.047 | 3.084 |
| cold labels: peak RSS MiB | 7.867 | 5.586 | 5.609 |
| cold labels: CPU ms/12 | 76.143 | 22.897 | 23.380 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 68.000 | 68.000 | 68.000 |
| cold expand: p50 ms | 8.457 | 1.342 | 1.296 |
| cold expand: p95 ms | 9.291 | 1.541 | 1.520 |
| cold expand: peak RSS MiB | 7.230 | 5.539 | 5.539 |
| cold expand: CPU ms/12 | 102.572 | 15.262 | 14.908 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 60.000 | 60.000 | 60.000 |
| cold children: p50 ms | 7.446 | 1.198 | 1.298 |
| cold children: p95 ms | 8.752 | 2.020 | 2.223 |
| cold children: peak RSS MiB | 6.266 | 5.523 | 5.512 |
| cold children: CPU ms/12 | 91.958 | 15.129 | 15.182 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 2.000 | 2.000 | 2.000 |
| cold logs: p50 ms | 0.843 | 0.195 | 0.196 |
| cold logs: p95 ms | 0.994 | 0.292 | 0.304 |
| cold logs: peak RSS MiB | 5.098 | 5.480 | 5.512 |
| cold logs: CPU ms/12 | 11.295 | 2.962 | 2.980 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 12.000 | 12.000 | 12.000 |
| cold relations: p50 ms | 5.614 | 0.183 | 0.181 |
| cold relations: p95 ms | 6.884 | 0.287 | 0.274 |
| cold relations: peak RSS MiB | 6.105 | 5.566 | 5.535 |
| cold relations: CPU ms/12 | 69.260 | 2.917 | 2.908 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 0.993 | 1.135 | 1.148 |
| cold id: p95 ms | 1.879 | 1.721 | 1.514 |
| cold id: peak RSS MiB | 6.117 | 5.527 | 5.551 |
| cold id: CPU ms/12 | 12.981 | 11.754 | 11.601 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 1.520 | 1.877 | 1.902 |
| cold clock: p95 ms | 1.901 | 2.000 | 2.053 |
| cold clock: peak RSS MiB | 5.824 | 5.555 | 5.559 |
| cold clock: CPU ms/12 | 20.153 | 23.604 | 23.742 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 3.527 | 0.028 | 0.028 |
| warm track: p95 ms | 4.484 | 0.058 | 0.057 |
| warm track: peak RSS MiB | 6.141 | 5.531 | 5.578 |
| warm track: CPU ms/12 | 38.255 | 1.385 | 1.364 |
| warm track: open ms | 0.031 | 0.077 | 0.072 |
| warm track: matches/12 | 34.000 | 34.000 | 34.000 |
| warm labels: p50 ms | 5.826 | 1.558 | 1.486 |
| warm labels: p95 ms | 11.133 | 3.415 | 2.966 |
| warm labels: peak RSS MiB | 7.883 | 5.586 | 5.617 |
| warm labels: CPU ms/12 | 76.054 | 23.504 | 22.158 |
| warm labels: open ms | 0.034 | 0.068 | 0.071 |
| warm labels: matches/12 | 68.000 | 68.000 | 68.000 |
| warm expand: p50 ms | 8.402 | 1.290 | 1.300 |
| warm expand: p95 ms | 9.197 | 1.395 | 1.476 |
| warm expand: peak RSS MiB | 7.273 | 5.547 | 5.531 |
| warm expand: CPU ms/12 | 102.470 | 14.550 | 14.608 |
| warm expand: open ms | 0.035 | 0.073 | 0.071 |
| warm expand: matches/12 | 60.000 | 60.000 | 60.000 |
| warm children: p50 ms | 7.382 | 1.209 | 1.187 |
| warm children: p95 ms | 8.583 | 1.932 | 1.923 |
| warm children: peak RSS MiB | 6.270 | 5.535 | 5.559 |
| warm children: CPU ms/12 | 91.217 | 14.841 | 14.735 |
| warm children: open ms | 0.035 | 0.078 | 0.072 |
| warm children: matches/12 | 2.000 | 2.000 | 2.000 |
| warm logs: p50 ms | 0.840 | 0.172 | 0.171 |
| warm logs: p95 ms | 0.960 | 0.208 | 0.210 |
| warm logs: peak RSS MiB | 5.098 | 5.473 | 5.531 |
| warm logs: CPU ms/12 | 11.320 | 2.668 | 2.683 |
| warm logs: open ms | 0.035 | 0.070 | 0.077 |
| warm logs: matches/12 | 12.000 | 12.000 | 12.000 |
| warm relations: p50 ms | 5.563 | 0.153 | 0.149 |
| warm relations: p95 ms | 6.828 | 0.198 | 0.199 |
| warm relations: peak RSS MiB | 7.227 | 5.570 | 5.555 |
| warm relations: CPU ms/12 | 69.574 | 2.542 | 2.551 |
| warm relations: open ms | 0.033 | 0.071 | 0.072 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 1.005 | 1.095 | 1.067 |
| warm id: p95 ms | 1.838 | 1.723 | 1.475 |
| warm id: peak RSS MiB | 7.281 | 5.539 | 5.488 |
| warm id: CPU ms/12 | 13.751 | 11.813 | 11.453 |
| warm id: open ms | 0.035 | 0.066 | 0.073 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 1.472 | 1.869 | 1.881 |
| warm clock: p95 ms | 1.867 | 2.063 | 1.990 |
| warm clock: peak RSS MiB | 5.727 | 5.555 | 5.543 |
| warm clock: CPU ms/12 | 19.375 | 23.826 | 23.654 |
| warm clock: open ms | 0.035 | 0.075 | 0.074 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |

### longtail

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 18.444 | 22.129 | 29.024 |
| write CPU ms | 19.213 | 22.915 | 29.847 |
| write max peak RSS MiB | 9.695 | 13.469 | 23.977 |
| read wall ms | 13.858 | 7.557 | 7.538 |
| read CPU ms | 14.837 | 8.566 | 8.559 |
| read max peak RSS MiB | 6.555 | 5.574 | 5.516 |
| null wall ms | 0.620 | 0.629 | 0.623 |
| null CPU ms | 1.480 | 1.538 | 1.672 |
| null max peak RSS MiB | 4.699 | 4.723 | 4.738 |
| write capture ms | 17.801 | 20.777 | 26.859 |
| write close ms | 0.539 | 1.119 | 1.927 |
| write file MiB | 0.918 | 0.546 | 0.546 |
| cold track: p50 ms | 2.388 | 0.050 | 0.050 |
| cold track: p95 ms | 2.992 | 0.151 | 0.152 |
| cold track: peak RSS MiB | 6.480 | 5.566 | 5.566 |
| cold track: CPU ms/12 | 30.669 | 1.673 | 1.616 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 38.000 | 38.000 | 38.000 |
| cold labels: p50 ms | 3.667 | 0.070 | 0.069 |
| cold labels: p95 ms | 4.434 | 0.179 | 0.180 |
| cold labels: peak RSS MiB | 6.746 | 5.645 | 5.613 |
| cold labels: CPU ms/12 | 43.552 | 1.772 | 1.766 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 76.000 | 76.000 | 76.000 |
| cold expand: p50 ms | 22.652 | 14.439 | 14.375 |
| cold expand: p95 ms | 39.032 | 20.126 | 20.297 |
| cold expand: peak RSS MiB | 50.938 | 22.914 | 22.902 |
| cold expand: CPU ms/12 | 293.689 | 182.421 | 182.045 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 1440012.000 | 1440012.000 | 1440012.000 |
| cold children: p50 ms | 9.268 | 0.389 | 0.395 |
| cold children: p95 ms | 11.204 | 0.578 | 0.570 |
| cold children: peak RSS MiB | 7.375 | 6.203 | 6.176 |
| cold children: CPU ms/12 | 113.961 | 5.812 | 5.904 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 22500.000 | 22500.000 | 22500.000 |
| cold logs: p50 ms | 3.115 | 2.046 | 2.073 |
| cold logs: p95 ms | 6.119 | 3.537 | 3.591 |
| cold logs: peak RSS MiB | 14.180 | 9.727 | 9.840 |
| cold logs: CPU ms/12 | 41.392 | 27.352 | 28.066 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 360000.000 | 360000.000 | 360000.000 |
| cold relations: p50 ms | 6.256 | 0.163 | 0.183 |
| cold relations: p95 ms | 7.802 | 0.273 | 0.284 |
| cold relations: peak RSS MiB | 6.598 | 5.605 | 5.539 |
| cold relations: CPU ms/12 | 78.453 | 2.898 | 3.160 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 0.615 | 0.091 | 0.090 |
| cold id: p95 ms | 1.900 | 1.030 | 1.031 |
| cold id: peak RSS MiB | 6.488 | 5.477 | 5.555 |
| cold id: CPU ms/12 | 11.508 | 3.774 | 3.852 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 0.364 | 0.174 | 0.176 |
| cold clock: p95 ms | 0.503 | 0.264 | 0.271 |
| cold clock: peak RSS MiB | 5.379 | 5.547 | 5.516 |
| cold clock: CPU ms/12 | 5.541 | 3.027 | 3.073 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 2.329 | 0.028 | 0.028 |
| warm track: p95 ms | 3.007 | 0.060 | 0.058 |
| warm track: peak RSS MiB | 6.555 | 5.562 | 5.574 |
| warm track: CPU ms/12 | 29.720 | 1.280 | 1.285 |
| warm track: open ms | 0.034 | 0.072 | 0.072 |
| warm track: matches/12 | 38.000 | 38.000 | 38.000 |
| warm labels: p50 ms | 3.625 | 0.028 | 0.028 |
| warm labels: p95 ms | 4.418 | 0.096 | 0.099 |
| warm labels: peak RSS MiB | 6.812 | 5.605 | 5.598 |
| warm labels: CPU ms/12 | 42.888 | 1.312 | 1.288 |
| warm labels: open ms | 0.037 | 0.073 | 0.072 |
| warm labels: matches/12 | 76.000 | 76.000 | 76.000 |
| warm expand: p50 ms | 22.535 | 14.241 | 14.208 |
| warm expand: p95 ms | 38.352 | 20.383 | 20.044 |
| warm expand: peak RSS MiB | 50.902 | 22.914 | 22.836 |
| warm expand: CPU ms/12 | 288.927 | 180.674 | 180.408 |
| warm expand: open ms | 0.037 | 0.071 | 0.074 |
| warm expand: matches/12 | 1440012.000 | 1440012.000 | 1440012.000 |
| warm children: p50 ms | 9.657 | 0.367 | 0.379 |
| warm children: p95 ms | 11.047 | 0.466 | 0.482 |
| warm children: peak RSS MiB | 8.645 | 6.133 | 6.168 |
| warm children: CPU ms/12 | 118.712 | 5.568 | 5.809 |
| warm children: open ms | 0.045 | 0.086 | 0.085 |
| warm children: matches/12 | 22500.000 | 22500.000 | 22500.000 |
| warm logs: p50 ms | 2.971 | 2.005 | 2.004 |
| warm logs: p95 ms | 6.015 | 3.497 | 3.493 |
| warm logs: peak RSS MiB | 14.062 | 9.746 | 9.840 |
| warm logs: CPU ms/12 | 39.948 | 27.332 | 27.319 |
| warm logs: open ms | 0.041 | 0.082 | 0.080 |
| warm logs: matches/12 | 360000.000 | 360000.000 | 360000.000 |
| warm relations: p50 ms | 6.354 | 0.155 | 0.146 |
| warm relations: p95 ms | 7.718 | 0.187 | 0.198 |
| warm relations: peak RSS MiB | 6.555 | 5.512 | 5.590 |
| warm relations: CPU ms/12 | 79.418 | 2.905 | 2.859 |
| warm relations: open ms | 0.041 | 0.088 | 0.086 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 0.591 | 0.039 | 0.039 |
| warm id: p95 ms | 1.886 | 1.038 | 1.000 |
| warm id: peak RSS MiB | 6.598 | 5.590 | 5.484 |
| warm id: CPU ms/12 | 11.222 | 3.608 | 3.490 |
| warm id: open ms | 0.040 | 0.075 | 0.076 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 0.347 | 0.152 | 0.147 |
| warm clock: p95 ms | 0.454 | 0.182 | 0.183 |
| warm clock: peak RSS MiB | 5.305 | 5.566 | 5.598 |
| warm clock: CPU ms/12 | 5.325 | 2.776 | 2.802 |
| warm clock: open ms | 0.038 | 0.075 | 0.077 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |

### logburst

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 16.061 | 30.380 | 34.846 |
| write CPU ms | 16.954 | 31.224 | 35.671 |
| write max peak RSS MiB | 9.863 | 15.480 | 20.410 |
| read wall ms | 22.621 | 10.517 | 10.814 |
| read CPU ms | 23.641 | 11.519 | 11.829 |
| read max peak RSS MiB | 5.746 | 5.570 | 5.566 |
| null wall ms | 0.327 | 0.322 | 0.333 |
| null CPU ms | 1.333 | 1.259 | 1.276 |
| null max peak RSS MiB | 4.684 | 4.699 | 4.719 |
| write capture ms | 15.613 | 28.461 | 29.216 |
| write close ms | 0.341 | 1.695 | 5.421 |
| write file MiB | 0.248 | 0.300 | 0.300 |
| cold track: p50 ms | 2.820 | 0.034 | 0.035 |
| cold track: p95 ms | 3.544 | 0.135 | 0.151 |
| cold track: peak RSS MiB | 6.059 | 5.574 | 5.621 |
| cold track: CPU ms/12 | 29.807 | 1.465 | 1.484 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 30.000 | 30.000 | 30.000 |
| cold labels: p50 ms | 5.471 | 0.673 | 0.662 |
| cold labels: p95 ms | 7.334 | 1.346 | 1.383 |
| cold labels: peak RSS MiB | 7.062 | 5.586 | 5.574 |
| cold labels: CPU ms/12 | 58.373 | 9.696 | 9.755 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 60.000 | 60.000 | 60.000 |
| cold expand: p50 ms | 17.583 | 0.400 | 0.407 |
| cold expand: p95 ms | 18.584 | 0.478 | 0.489 |
| cold expand: peak RSS MiB | 7.316 | 5.570 | 5.555 |
| cold expand: CPU ms/12 | 211.723 | 5.254 | 5.316 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 60.000 | 60.000 | 60.000 |
| cold children: p50 ms | 16.507 | 0.680 | 0.685 |
| cold children: p95 ms | 17.339 | 1.071 | 1.107 |
| cold children: peak RSS MiB | 6.445 | 5.590 | 5.598 |
| cold children: CPU ms/12 | 199.625 | 8.883 | 9.083 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 1.000 | 1.000 | 1.000 |
| cold logs: p50 ms | 6.976 | 0.214 | 0.216 |
| cold logs: p95 ms | 8.968 | 0.323 | 0.323 |
| cold logs: peak RSS MiB | 5.344 | 5.516 | 5.617 |
| cold logs: CPU ms/12 | 85.140 | 3.650 | 3.681 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 192.000 | 192.000 | 192.000 |
| cold relations: p50 ms | 2.802 | 0.189 | 0.197 |
| cold relations: p95 ms | 3.509 | 0.290 | 0.301 |
| cold relations: peak RSS MiB | 6.113 | 5.582 | 5.590 |
| cold relations: CPU ms/12 | 35.484 | 3.286 | 3.370 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 0.772 | 0.234 | 0.239 |
| cold id: p95 ms | 1.872 | 0.668 | 0.667 |
| cold id: peak RSS MiB | 6.020 | 5.609 | 5.605 |
| cold id: CPU ms/12 | 12.018 | 4.621 | 4.637 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 0.752 | 0.979 | 0.973 |
| cold clock: p95 ms | 0.964 | 1.085 | 1.064 |
| cold clock: peak RSS MiB | 5.352 | 5.523 | 5.516 |
| cold clock: CPU ms/12 | 10.466 | 12.772 | 12.745 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 2.816 | 0.011 | 0.011 |
| warm track: p95 ms | 3.598 | 0.048 | 0.048 |
| warm track: peak RSS MiB | 6.109 | 5.605 | 5.590 |
| warm track: CPU ms/12 | 29.718 | 1.155 | 1.189 |
| warm track: open ms | 0.029 | 0.079 | 0.079 |
| warm track: matches/12 | 30.000 | 30.000 | 30.000 |
| warm labels: p50 ms | 5.403 | 0.632 | 0.635 |
| warm labels: p95 ms | 7.367 | 1.291 | 1.283 |
| warm labels: peak RSS MiB | 6.926 | 5.590 | 5.527 |
| warm labels: CPU ms/12 | 58.445 | 9.308 | 9.206 |
| warm labels: open ms | 0.033 | 0.084 | 0.074 |
| warm labels: matches/12 | 60.000 | 60.000 | 60.000 |
| warm expand: p50 ms | 17.675 | 0.377 | 0.373 |
| warm expand: p95 ms | 18.674 | 0.472 | 0.461 |
| warm expand: peak RSS MiB | 7.215 | 5.570 | 5.547 |
| warm expand: CPU ms/12 | 213.345 | 4.972 | 4.956 |
| warm expand: open ms | 0.034 | 0.078 | 0.078 |
| warm expand: matches/12 | 60.000 | 60.000 | 60.000 |
| warm children: p50 ms | 16.642 | 0.648 | 0.650 |
| warm children: p95 ms | 17.729 | 0.989 | 0.986 |
| warm children: peak RSS MiB | 6.547 | 5.590 | 5.566 |
| warm children: CPU ms/12 | 202.304 | 8.643 | 8.554 |
| warm children: open ms | 0.037 | 0.077 | 0.076 |
| warm children: matches/12 | 1.000 | 1.000 | 1.000 |
| warm logs: p50 ms | 7.035 | 0.197 | 0.193 |
| warm logs: p95 ms | 7.481 | 0.287 | 0.279 |
| warm logs: peak RSS MiB | 5.727 | 5.602 | 5.586 |
| warm logs: CPU ms/12 | 86.447 | 3.386 | 3.428 |
| warm logs: open ms | 0.034 | 0.077 | 0.077 |
| warm logs: matches/12 | 192.000 | 192.000 | 192.000 |
| warm relations: p50 ms | 2.781 | 0.163 | 0.166 |
| warm relations: p95 ms | 3.608 | 0.205 | 0.201 |
| warm relations: peak RSS MiB | 6.098 | 5.570 | 5.566 |
| warm relations: CPU ms/12 | 35.866 | 3.088 | 2.988 |
| warm relations: open ms | 0.033 | 0.079 | 0.078 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 0.764 | 0.220 | 0.215 |
| warm id: p95 ms | 1.835 | 0.641 | 0.633 |
| warm id: peak RSS MiB | 6.113 | 5.598 | 5.605 |
| warm id: CPU ms/12 | 11.719 | 4.365 | 4.311 |
| warm id: open ms | 0.031 | 0.083 | 0.079 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 0.749 | 0.957 | 0.946 |
| warm clock: p95 ms | 0.948 | 1.082 | 0.999 |
| warm clock: peak RSS MiB | 5.348 | 5.477 | 5.570 |
| warm clock: CPU ms/12 | 10.175 | 12.564 | 12.484 |
| warm clock: open ms | 0.033 | 0.081 | 0.082 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |

### disordered

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 15.263 | 27.757 | 32.911 |
| write CPU ms | 16.101 | 28.631 | 33.711 |
| write max peak RSS MiB | 9.500 | 15.566 | 25.633 |
| read wall ms | 11.133 | 7.703 | 7.819 |
| read CPU ms | 12.146 | 8.698 | 8.878 |
| read max peak RSS MiB | 6.113 | 5.539 | 5.555 |
| null wall ms | 0.436 | 0.432 | 0.432 |
| null CPU ms | 1.412 | 1.340 | 1.378 |
| null max peak RSS MiB | 4.672 | 4.648 | 4.691 |
| write capture ms | 14.450 | 26.481 | 30.632 |
| write close ms | 0.706 | 1.043 | 2.031 |
| write file MiB | 0.413 | 0.493 | 0.493 |
| cold track: p50 ms | 5.734 | 0.095 | 0.096 |
| cold track: p95 ms | 6.611 | 0.154 | 0.150 |
| cold track: peak RSS MiB | 6.113 | 5.551 | 5.543 |
| cold track: CPU ms/12 | 70.900 | 2.166 | 2.161 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 40.000 | 40.000 | 40.000 |
| cold labels: p50 ms | 10.011 | 3.068 | 3.128 |
| cold labels: p95 ms | 14.225 | 3.379 | 3.507 |
| cold labels: peak RSS MiB | 7.168 | 5.633 | 5.570 |
| cold labels: CPU ms/12 | 127.959 | 31.840 | 32.419 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 80.000 | 80.000 | 80.000 |
| cold expand: p50 ms | 8.429 | 1.384 | 1.376 |
| cold expand: p95 ms | 9.230 | 1.481 | 1.481 |
| cold expand: peak RSS MiB | 7.348 | 5.562 | 5.594 |
| cold expand: CPU ms/12 | 102.069 | 15.636 | 15.488 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 60.000 | 60.000 | 60.000 |
| cold children: p50 ms | 7.416 | 1.191 | 1.190 |
| cold children: p95 ms | 8.387 | 1.956 | 1.953 |
| cold children: peak RSS MiB | 6.387 | 5.527 | 5.535 |
| cold children: CPU ms/12 | 91.375 | 14.722 | 14.591 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 2.000 | 2.000 | 2.000 |
| cold logs: p50 ms | 0.842 | 0.214 | 0.211 |
| cold logs: p95 ms | 0.994 | 0.315 | 0.306 |
| cold logs: peak RSS MiB | 5.164 | 5.598 | 5.523 |
| cold logs: CPU ms/12 | 11.220 | 3.217 | 3.209 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 12.000 | 12.000 | 12.000 |
| cold relations: p50 ms | 5.584 | 0.160 | 0.157 |
| cold relations: p95 ms | 6.625 | 0.303 | 0.293 |
| cold relations: peak RSS MiB | 6.133 | 5.562 | 5.594 |
| cold relations: CPU ms/12 | 69.227 | 2.937 | 2.808 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 1.005 | 1.158 | 1.147 |
| cold id: p95 ms | 1.852 | 1.641 | 1.656 |
| cold id: peak RSS MiB | 6.152 | 5.520 | 5.594 |
| cold id: CPU ms/12 | 13.065 | 12.295 | 12.301 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 1.483 | 1.872 | 1.842 |
| cold clock: p95 ms | 1.907 | 2.067 | 1.951 |
| cold clock: peak RSS MiB | 5.812 | 5.539 | 5.562 |
| cold clock: CPU ms/12 | 19.552 | 23.800 | 23.129 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 5.597 | 0.073 | 0.073 |
| warm track: p95 ms | 6.586 | 0.075 | 0.079 |
| warm track: peak RSS MiB | 6.125 | 5.551 | 5.562 |
| warm track: CPU ms/12 | 69.315 | 1.858 | 1.860 |
| warm track: open ms | 0.032 | 0.070 | 0.070 |
| warm track: matches/12 | 40.000 | 40.000 | 40.000 |
| warm labels: p50 ms | 9.982 | 3.001 | 3.044 |
| warm labels: p95 ms | 14.227 | 3.417 | 3.492 |
| warm labels: peak RSS MiB | 7.289 | 5.676 | 5.684 |
| warm labels: CPU ms/12 | 127.845 | 30.823 | 31.602 |
| warm labels: open ms | 0.034 | 0.069 | 0.072 |
| warm labels: matches/12 | 80.000 | 80.000 | 80.000 |
| warm expand: p50 ms | 8.441 | 1.425 | 1.370 |
| warm expand: p95 ms | 9.431 | 1.557 | 1.445 |
| warm expand: peak RSS MiB | 7.348 | 5.539 | 5.516 |
| warm expand: CPU ms/12 | 103.732 | 16.033 | 15.535 |
| warm expand: open ms | 0.036 | 0.076 | 0.076 |
| warm expand: matches/12 | 60.000 | 60.000 | 60.000 |
| warm children: p50 ms | 7.426 | 1.173 | 1.159 |
| warm children: p95 ms | 8.300 | 1.965 | 1.898 |
| warm children: peak RSS MiB | 6.379 | 5.477 | 5.512 |
| warm children: CPU ms/12 | 91.742 | 14.465 | 14.385 |
| warm children: open ms | 0.037 | 0.070 | 0.075 |
| warm children: matches/12 | 2.000 | 2.000 | 2.000 |
| warm logs: p50 ms | 0.815 | 0.189 | 0.190 |
| warm logs: p95 ms | 0.949 | 0.224 | 0.224 |
| warm logs: peak RSS MiB | 5.160 | 5.582 | 5.582 |
| warm logs: CPU ms/12 | 10.939 | 2.938 | 2.981 |
| warm logs: open ms | 0.035 | 0.074 | 0.076 |
| warm logs: matches/12 | 12.000 | 12.000 | 12.000 |
| warm relations: p50 ms | 5.624 | 0.127 | 0.137 |
| warm relations: p95 ms | 6.552 | 0.214 | 0.213 |
| warm relations: peak RSS MiB | 7.324 | 5.574 | 5.559 |
| warm relations: CPU ms/12 | 69.666 | 2.446 | 2.590 |
| warm relations: open ms | 0.034 | 0.072 | 0.071 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 0.983 | 1.122 | 1.130 |
| warm id: p95 ms | 1.825 | 1.592 | 1.652 |
| warm id: peak RSS MiB | 7.312 | 5.512 | 5.500 |
| warm id: CPU ms/12 | 13.348 | 11.973 | 12.070 |
| warm id: open ms | 0.035 | 0.070 | 0.069 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 1.470 | 1.816 | 1.821 |
| warm clock: p95 ms | 1.883 | 1.883 | 1.976 |
| warm clock: peak RSS MiB | 5.742 | 5.527 | 5.496 |
| warm clock: CPU ms/12 | 19.473 | 22.842 | 23.048 |
| warm clock: open ms | 0.036 | 0.069 | 0.072 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |

### large

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 78.696 | 47.312 | 50.361 |
| write CPU ms | 79.558 | 48.097 | 51.170 |
| write max peak RSS MiB | 28.816 | 14.754 | 19.188 |
| read wall ms | 60.092 | 54.284 | 54.098 |
| read CPU ms | 61.093 | 55.311 | 55.091 |
| read max peak RSS MiB | 12.777 | 5.625 | 5.617 |
| null wall ms | 0.185 | 0.174 | 0.176 |
| null CPU ms | 1.167 | 1.116 | 1.144 |
| null max peak RSS MiB | 4.680 | 4.730 | 4.746 |
| write capture ms | 76.787 | 46.455 | 47.813 |
| write close ms | 1.797 | 0.632 | 2.269 |
| write file MiB | 0.295 | 0.380 | 0.380 |
| cold track: p50 ms | 1.733 | 0.063 | 0.064 |
| cold track: p95 ms | 4.093 | 0.193 | 0.192 |
| cold track: peak RSS MiB | 8.836 | 5.582 | 5.676 |
| cold track: CPU ms/12 | 28.503 | 1.852 | 1.863 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 40.000 | 40.000 | 40.000 |
| cold labels: p50 ms | 7.955 | 0.982 | 0.980 |
| cold labels: p95 ms | 11.830 | 1.457 | 1.471 |
| cold labels: peak RSS MiB | 12.598 | 5.656 | 5.777 |
| cold labels: CPU ms/12 | 96.094 | 13.087 | 13.048 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 80.000 | 80.000 | 80.000 |
| cold expand: p50 ms | 15.582 | 0.710 | 0.706 |
| cold expand: p95 ms | 20.164 | 0.740 | 0.733 |
| cold expand: peak RSS MiB | 12.848 | 5.734 | 5.703 |
| cold expand: CPU ms/12 | 191.231 | 8.320 | 8.260 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 60.000 | 60.000 | 60.000 |
| cold children: p50 ms | 19.700 | 1.734 | 1.709 |
| cold children: p95 ms | 21.608 | 3.252 | 3.275 |
| cold children: peak RSS MiB | 12.609 | 5.637 | 5.629 |
| cold children: CPU ms/12 | 238.393 | 21.481 | 21.769 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 1.000 | 1.000 | 1.000 |
| cold logs: p50 ms | 2.793 | 0.066 | 0.064 |
| cold logs: p95 ms | 7.391 | 0.187 | 0.188 |
| cold logs: peak RSS MiB | 12.805 | 5.645 | 5.695 |
| cold logs: CPU ms/12 | 41.304 | 1.869 | 1.840 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 12.000 | 12.000 | 12.000 |
| cold relations: p50 ms | 11.405 | 0.049 | 0.052 |
| cold relations: p95 ms | 14.655 | 0.151 | 0.158 |
| cold relations: peak RSS MiB | 8.879 | 5.648 | 5.609 |
| cold relations: CPU ms/12 | 141.569 | 1.574 | 1.584 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 0.816 | 0.475 | 0.463 |
| cold id: p95 ms | 2.392 | 0.745 | 0.734 |
| cold id: peak RSS MiB | 8.914 | 5.703 | 5.594 |
| cold id: CPU ms/12 | 12.745 | 6.096 | 6.069 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 0.494 | 3.417 | 3.434 |
| cold clock: p95 ms | 1.363 | 3.631 | 3.643 |
| cold clock: peak RSS MiB | 6.781 | 5.609 | 5.609 |
| cold clock: CPU ms/12 | 8.424 | 42.587 | 42.190 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 1.616 | 0.024 | 0.024 |
| warm track: p95 ms | 4.088 | 0.057 | 0.054 |
| warm track: peak RSS MiB | 8.840 | 5.590 | 5.629 |
| warm track: CPU ms/12 | 27.750 | 1.374 | 1.375 |
| warm track: open ms | 0.044 | 0.113 | 0.118 |
| warm track: matches/12 | 40.000 | 40.000 | 40.000 |
| warm labels: p50 ms | 7.875 | 0.908 | 0.937 |
| warm labels: p95 ms | 11.828 | 1.553 | 1.463 |
| warm labels: peak RSS MiB | 12.609 | 5.875 | 5.832 |
| warm labels: CPU ms/12 | 95.334 | 12.871 | 12.747 |
| warm labels: open ms | 0.044 | 0.112 | 0.117 |
| warm labels: matches/12 | 80.000 | 80.000 | 80.000 |
| warm expand: p50 ms | 15.305 | 0.662 | 0.664 |
| warm expand: p95 ms | 20.157 | 0.710 | 0.687 |
| warm expand: peak RSS MiB | 12.844 | 5.742 | 5.766 |
| warm expand: CPU ms/12 | 190.007 | 7.842 | 7.793 |
| warm expand: open ms | 0.046 | 0.115 | 0.126 |
| warm expand: matches/12 | 60.000 | 60.000 | 60.000 |
| warm children: p50 ms | 15.210 | 1.680 | 1.632 |
| warm children: p95 ms | 21.450 | 3.168 | 3.148 |
| warm children: peak RSS MiB | 12.898 | 5.656 | 5.676 |
| warm children: CPU ms/12 | 190.769 | 21.242 | 21.001 |
| warm children: open ms | 0.048 | 0.115 | 0.116 |
| warm children: matches/12 | 1.000 | 1.000 | 1.000 |
| warm logs: p50 ms | 2.691 | 0.032 | 0.030 |
| warm logs: p95 ms | 7.444 | 0.061 | 0.057 |
| warm logs: peak RSS MiB | 12.824 | 5.629 | 5.648 |
| warm logs: CPU ms/12 | 40.413 | 1.483 | 1.459 |
| warm logs: open ms | 0.045 | 0.129 | 0.118 |
| warm logs: matches/12 | 12.000 | 12.000 | 12.000 |
| warm relations: p50 ms | 11.326 | 0.015 | 0.015 |
| warm relations: p95 ms | 14.558 | 0.029 | 0.031 |
| warm relations: peak RSS MiB | 8.883 | 5.598 | 5.570 |
| warm relations: CPU ms/12 | 140.862 | 1.153 | 1.136 |
| warm relations: open ms | 0.046 | 0.116 | 0.115 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 0.788 | 0.425 | 0.423 |
| warm id: p95 ms | 2.349 | 0.678 | 0.673 |
| warm id: peak RSS MiB | 8.883 | 5.715 | 5.699 |
| warm id: CPU ms/12 | 12.422 | 5.726 | 5.699 |
| warm id: open ms | 0.048 | 0.115 | 0.112 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 0.480 | 3.358 | 3.352 |
| warm clock: p95 ms | 1.335 | 3.484 | 3.457 |
| warm clock: peak RSS MiB | 6.777 | 5.656 | 5.590 |
| warm clock: CPU ms/12 | 8.256 | 41.275 | 41.413 |
| warm clock: open ms | 0.045 | 0.114 | 0.117 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |


## Appendix F: retained navigation results, three CPUs

### balanced

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 15.196 | 27.205 | 36.667 |
| write CPU ms | 16.040 | 28.091 | 38.733 |
| write max peak RSS MiB | 9.449 | 15.621 | 25.211 |
| read wall ms | 11.533 | 7.402 | 7.560 |
| read CPU ms | 12.737 | 8.418 | 8.677 |
| read max peak RSS MiB | 6.031 | 5.594 | 5.555 |
| null wall ms | 0.451 | 0.425 | 0.431 |
| null CPU ms | 1.322 | 1.536 | 1.448 |
| null max peak RSS MiB | 4.660 | 4.715 | 4.707 |
| write capture ms | 14.432 | 25.965 | 30.052 |
| write close ms | 0.697 | 1.024 | 6.069 |
| write file MiB | 0.408 | 0.486 | 0.486 |
| cold track: p50 ms | 3.598 | 0.050 | 0.051 |
| cold track: p95 ms | 4.385 | 0.125 | 0.128 |
| cold track: peak RSS MiB | 6.117 | 5.535 | 5.582 |
| cold track: CPU ms/12 | 38.957 | 1.512 | 1.644 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 34.000 | 34.000 | 34.000 |
| cold labels: p50 ms | 5.802 | 1.540 | 1.552 |
| cold labels: p95 ms | 11.040 | 2.979 | 3.115 |
| cold labels: peak RSS MiB | 7.969 | 5.578 | 5.586 |
| cold labels: CPU ms/12 | 75.283 | 22.710 | 23.220 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 68.000 | 68.000 | 68.000 |
| cold expand: p50 ms | 8.537 | 1.308 | 1.336 |
| cold expand: p95 ms | 9.311 | 1.478 | 1.460 |
| cold expand: peak RSS MiB | 7.320 | 5.562 | 5.594 |
| cold expand: CPU ms/12 | 104.528 | 15.018 | 15.095 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 60.000 | 60.000 | 60.000 |
| cold children: p50 ms | 7.454 | 1.224 | 1.212 |
| cold children: p95 ms | 8.738 | 1.995 | 1.997 |
| cold children: peak RSS MiB | 6.336 | 5.516 | 5.582 |
| cold children: CPU ms/12 | 91.868 | 15.248 | 15.096 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 2.000 | 2.000 | 2.000 |
| cold logs: p50 ms | 0.850 | 0.203 | 0.196 |
| cold logs: p95 ms | 0.980 | 0.279 | 0.283 |
| cold logs: peak RSS MiB | 5.082 | 5.551 | 5.551 |
| cold logs: CPU ms/12 | 11.440 | 2.911 | 2.968 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 12.000 | 12.000 | 12.000 |
| cold relations: p50 ms | 5.697 | 0.174 | 0.189 |
| cold relations: p95 ms | 7.063 | 0.264 | 0.267 |
| cold relations: peak RSS MiB | 6.098 | 5.582 | 5.547 |
| cold relations: CPU ms/12 | 70.500 | 2.946 | 2.778 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 0.998 | 1.111 | 1.086 |
| cold id: p95 ms | 1.893 | 1.555 | 1.525 |
| cold id: peak RSS MiB | 6.145 | 5.516 | 5.566 |
| cold id: CPU ms/12 | 12.972 | 11.777 | 11.455 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 1.503 | 1.919 | 1.932 |
| cold clock: p95 ms | 1.877 | 2.040 | 2.016 |
| cold clock: peak RSS MiB | 5.832 | 5.531 | 5.566 |
| cold clock: CPU ms/12 | 19.671 | 23.854 | 24.152 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 3.597 | 0.028 | 0.028 |
| warm track: p95 ms | 4.423 | 0.053 | 0.052 |
| warm track: peak RSS MiB | 6.105 | 5.508 | 5.582 |
| warm track: CPU ms/12 | 38.621 | 1.382 | 1.258 |
| warm track: open ms | 0.029 | 0.063 | 0.064 |
| warm track: matches/12 | 34.000 | 34.000 | 34.000 |
| warm labels: p50 ms | 5.771 | 1.508 | 1.495 |
| warm labels: p95 ms | 11.099 | 3.074 | 3.077 |
| warm labels: peak RSS MiB | 8.020 | 5.598 | 5.547 |
| warm labels: CPU ms/12 | 75.533 | 22.585 | 22.505 |
| warm labels: open ms | 0.033 | 0.058 | 0.060 |
| warm labels: matches/12 | 68.000 | 68.000 | 68.000 |
| warm expand: p50 ms | 8.397 | 1.313 | 1.282 |
| warm expand: p95 ms | 9.202 | 1.438 | 1.538 |
| warm expand: peak RSS MiB | 7.316 | 5.520 | 5.508 |
| warm expand: CPU ms/12 | 102.246 | 14.737 | 14.576 |
| warm expand: open ms | 0.036 | 0.064 | 0.061 |
| warm expand: matches/12 | 60.000 | 60.000 | 60.000 |
| warm children: p50 ms | 7.454 | 1.168 | 1.178 |
| warm children: p95 ms | 8.791 | 1.921 | 1.905 |
| warm children: peak RSS MiB | 6.258 | 5.547 | 5.578 |
| warm children: CPU ms/12 | 92.503 | 14.746 | 14.831 |
| warm children: open ms | 0.034 | 0.065 | 0.064 |
| warm children: matches/12 | 2.000 | 2.000 | 2.000 |
| warm logs: p50 ms | 0.813 | 0.177 | 0.174 |
| warm logs: p95 ms | 0.921 | 0.220 | 0.249 |
| warm logs: peak RSS MiB | 5.113 | 5.535 | 5.586 |
| warm logs: CPU ms/12 | 11.047 | 2.802 | 2.725 |
| warm logs: open ms | 0.036 | 0.063 | 0.064 |
| warm logs: matches/12 | 12.000 | 12.000 | 12.000 |
| warm relations: p50 ms | 5.667 | 0.147 | 0.162 |
| warm relations: p95 ms | 6.786 | 0.196 | 0.232 |
| warm relations: peak RSS MiB | 7.219 | 5.559 | 5.578 |
| warm relations: CPU ms/12 | 70.909 | 2.460 | 2.556 |
| warm relations: open ms | 0.031 | 0.065 | 0.064 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 1.034 | 1.061 | 1.082 |
| warm id: p95 ms | 1.865 | 1.510 | 1.609 |
| warm id: peak RSS MiB | 7.270 | 5.547 | 5.539 |
| warm id: CPU ms/12 | 13.802 | 11.360 | 11.561 |
| warm id: open ms | 0.036 | 0.062 | 0.062 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 1.487 | 1.878 | 1.860 |
| warm clock: p95 ms | 1.860 | 2.024 | 2.041 |
| warm clock: peak RSS MiB | 5.758 | 5.484 | 5.535 |
| warm clock: CPU ms/12 | 19.334 | 23.555 | 23.474 |
| warm clock: open ms | 0.033 | 0.063 | 0.063 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |

### longtail

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 18.449 | 22.275 | 29.005 |
| write CPU ms | 19.222 | 22.954 | 30.369 |
| write max peak RSS MiB | 9.754 | 13.445 | 21.578 |
| read wall ms | 13.887 | 7.590 | 7.677 |
| read CPU ms | 14.893 | 8.693 | 8.669 |
| read max peak RSS MiB | 6.555 | 5.598 | 5.574 |
| null wall ms | 0.640 | 0.650 | 0.615 |
| null CPU ms | 1.364 | 1.618 | 1.738 |
| null max peak RSS MiB | 4.672 | 4.711 | 4.746 |
| write capture ms | 17.816 | 20.922 | 24.726 |
| write close ms | 0.534 | 1.119 | 4.460 |
| write file MiB | 0.918 | 0.546 | 0.546 |
| cold track: p50 ms | 2.387 | 0.050 | 0.050 |
| cold track: p95 ms | 3.080 | 0.136 | 0.139 |
| cold track: peak RSS MiB | 6.535 | 5.527 | 5.559 |
| cold track: CPU ms/12 | 30.517 | 1.537 | 1.662 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 38.000 | 38.000 | 38.000 |
| cold labels: p50 ms | 3.678 | 0.072 | 0.074 |
| cold labels: p95 ms | 4.437 | 0.173 | 0.174 |
| cold labels: peak RSS MiB | 6.707 | 5.633 | 5.652 |
| cold labels: CPU ms/12 | 43.584 | 1.761 | 1.814 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 76.000 | 76.000 | 76.000 |
| cold expand: p50 ms | 22.602 | 14.532 | 14.626 |
| cold expand: p95 ms | 38.043 | 20.379 | 20.302 |
| cold expand: peak RSS MiB | 50.844 | 22.887 | 22.898 |
| cold expand: CPU ms/12 | 291.028 | 182.349 | 182.896 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 1440012.000 | 1440012.000 | 1440012.000 |
| cold children: p50 ms | 9.249 | 0.396 | 0.392 |
| cold children: p95 ms | 11.095 | 0.569 | 0.555 |
| cold children: peak RSS MiB | 7.273 | 6.176 | 6.152 |
| cold children: CPU ms/12 | 113.966 | 6.028 | 5.928 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 22500.000 | 22500.000 | 22500.000 |
| cold logs: p50 ms | 3.018 | 2.042 | 2.056 |
| cold logs: p95 ms | 6.081 | 3.518 | 3.534 |
| cold logs: peak RSS MiB | 14.125 | 9.789 | 9.840 |
| cold logs: CPU ms/12 | 40.950 | 27.694 | 27.592 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 360000.000 | 360000.000 | 360000.000 |
| cold relations: p50 ms | 6.260 | 0.166 | 0.166 |
| cold relations: p95 ms | 7.814 | 0.270 | 0.259 |
| cold relations: peak RSS MiB | 6.523 | 5.531 | 5.562 |
| cold relations: CPU ms/12 | 78.035 | 3.012 | 2.850 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 0.611 | 0.084 | 0.083 |
| cold id: p95 ms | 1.896 | 1.030 | 1.024 |
| cold id: peak RSS MiB | 6.488 | 5.559 | 5.551 |
| cold id: CPU ms/12 | 11.542 | 3.826 | 3.828 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 0.364 | 0.176 | 0.173 |
| cold clock: p95 ms | 0.487 | 0.259 | 0.261 |
| cold clock: peak RSS MiB | 5.414 | 5.559 | 5.598 |
| cold clock: CPU ms/12 | 5.632 | 2.979 | 3.104 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 2.338 | 0.028 | 0.028 |
| warm track: p95 ms | 2.941 | 0.054 | 0.055 |
| warm track: peak RSS MiB | 6.590 | 5.566 | 5.574 |
| warm track: CPU ms/12 | 29.931 | 1.142 | 1.167 |
| warm track: open ms | 0.033 | 0.062 | 0.062 |
| warm track: matches/12 | 38.000 | 38.000 | 38.000 |
| warm labels: p50 ms | 3.667 | 0.028 | 0.028 |
| warm labels: p95 ms | 4.472 | 0.092 | 0.094 |
| warm labels: peak RSS MiB | 6.754 | 5.602 | 5.633 |
| warm labels: CPU ms/12 | 43.621 | 1.252 | 1.140 |
| warm labels: open ms | 0.038 | 0.062 | 0.062 |
| warm labels: matches/12 | 76.000 | 76.000 | 76.000 |
| warm expand: p50 ms | 22.367 | 14.365 | 14.368 |
| warm expand: p95 ms | 37.612 | 20.326 | 20.260 |
| warm expand: peak RSS MiB | 50.773 | 22.898 | 22.828 |
| warm expand: CPU ms/12 | 287.780 | 181.872 | 182.190 |
| warm expand: open ms | 0.040 | 0.066 | 0.063 |
| warm expand: matches/12 | 1440012.000 | 1440012.000 | 1440012.000 |
| warm children: p50 ms | 9.720 | 0.371 | 0.363 |
| warm children: p95 ms | 11.045 | 0.471 | 0.466 |
| warm children: peak RSS MiB | 8.637 | 6.141 | 6.090 |
| warm children: CPU ms/12 | 118.582 | 5.697 | 5.600 |
| warm children: open ms | 0.041 | 0.085 | 0.083 |
| warm children: matches/12 | 22500.000 | 22500.000 | 22500.000 |
| warm logs: p50 ms | 2.973 | 2.005 | 2.000 |
| warm logs: p95 ms | 6.057 | 3.476 | 3.441 |
| warm logs: peak RSS MiB | 14.070 | 9.836 | 9.883 |
| warm logs: CPU ms/12 | 40.436 | 27.091 | 27.440 |
| warm logs: open ms | 0.039 | 0.072 | 0.072 |
| warm logs: matches/12 | 360000.000 | 360000.000 | 360000.000 |
| warm relations: p50 ms | 6.190 | 0.152 | 0.153 |
| warm relations: p95 ms | 7.762 | 0.183 | 0.184 |
| warm relations: peak RSS MiB | 6.562 | 5.527 | 5.555 |
| warm relations: CPU ms/12 | 77.537 | 2.782 | 2.826 |
| warm relations: open ms | 0.041 | 0.083 | 0.078 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 0.577 | 0.037 | 0.037 |
| warm id: p95 ms | 1.874 | 1.002 | 1.011 |
| warm id: peak RSS MiB | 6.586 | 5.547 | 5.539 |
| warm id: CPU ms/12 | 11.270 | 3.629 | 3.528 |
| warm id: open ms | 0.039 | 0.073 | 0.069 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 0.342 | 0.144 | 0.147 |
| warm clock: p95 ms | 0.450 | 0.179 | 0.177 |
| warm clock: peak RSS MiB | 5.336 | 5.551 | 5.555 |
| warm clock: CPU ms/12 | 5.479 | 2.740 | 2.605 |
| warm clock: open ms | 0.038 | 0.070 | 0.065 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |

### logburst

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 16.022 | 30.204 | 33.199 |
| write CPU ms | 16.818 | 31.029 | 40.341 |
| write max peak RSS MiB | 9.840 | 15.625 | 26.555 |
| read wall ms | 22.279 | 10.792 | 11.042 |
| read CPU ms | 23.393 | 11.768 | 12.158 |
| read max peak RSS MiB | 5.926 | 5.547 | 5.559 |
| null wall ms | 0.323 | 0.324 | 0.323 |
| null CPU ms | 1.250 | 1.312 | 1.318 |
| null max peak RSS MiB | 4.664 | 4.703 | 4.719 |
| write capture ms | 15.585 | 28.332 | 25.875 |
| write close ms | 0.344 | 1.680 | 7.122 |
| write file MiB | 0.248 | 0.300 | 0.300 |
| cold track: p50 ms | 2.845 | 0.034 | 0.035 |
| cold track: p95 ms | 3.540 | 0.121 | 0.130 |
| cold track: peak RSS MiB | 6.102 | 5.621 | 5.551 |
| cold track: CPU ms/12 | 30.052 | 1.360 | 1.370 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 30.000 | 30.000 | 30.000 |
| cold labels: p50 ms | 5.609 | 0.671 | 0.679 |
| cold labels: p95 ms | 7.602 | 1.373 | 1.384 |
| cold labels: peak RSS MiB | 7.070 | 5.605 | 5.652 |
| cold labels: CPU ms/12 | 58.931 | 9.766 | 9.700 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 60.000 | 60.000 | 60.000 |
| cold expand: p50 ms | 17.703 | 0.411 | 0.406 |
| cold expand: p95 ms | 18.880 | 0.505 | 0.502 |
| cold expand: peak RSS MiB | 7.312 | 5.613 | 5.574 |
| cold expand: CPU ms/12 | 213.960 | 5.338 | 5.181 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 60.000 | 60.000 | 60.000 |
| cold children: p50 ms | 16.489 | 0.692 | 0.681 |
| cold children: p95 ms | 17.734 | 1.070 | 1.072 |
| cold children: peak RSS MiB | 6.512 | 5.602 | 5.555 |
| cold children: CPU ms/12 | 200.715 | 9.049 | 9.174 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 1.000 | 1.000 | 1.000 |
| cold logs: p50 ms | 6.918 | 0.218 | 0.222 |
| cold logs: p95 ms | 7.413 | 0.324 | 0.308 |
| cold logs: peak RSS MiB | 5.273 | 5.578 | 5.559 |
| cold logs: CPU ms/12 | 84.527 | 3.728 | 3.708 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 192.000 | 192.000 | 192.000 |
| cold relations: p50 ms | 2.843 | 0.186 | 0.197 |
| cold relations: p95 ms | 3.523 | 0.268 | 0.285 |
| cold relations: peak RSS MiB | 6.125 | 5.609 | 5.613 |
| cold relations: CPU ms/12 | 36.436 | 3.153 | 3.334 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 0.791 | 0.239 | 0.242 |
| cold id: p95 ms | 1.870 | 0.679 | 0.666 |
| cold id: peak RSS MiB | 6.094 | 5.582 | 5.617 |
| cold id: CPU ms/12 | 11.983 | 4.658 | 4.654 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 0.754 | 0.982 | 0.992 |
| cold clock: p95 ms | 0.964 | 1.064 | 1.059 |
| cold clock: peak RSS MiB | 5.340 | 5.551 | 5.594 |
| cold clock: CPU ms/12 | 10.336 | 12.809 | 12.624 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 2.778 | 0.011 | 0.011 |
| warm track: p95 ms | 3.465 | 0.045 | 0.045 |
| warm track: peak RSS MiB | 6.070 | 5.559 | 5.586 |
| warm track: CPU ms/12 | 29.353 | 1.062 | 1.063 |
| warm track: open ms | 0.027 | 0.064 | 0.063 |
| warm track: matches/12 | 30.000 | 30.000 | 30.000 |
| warm labels: p50 ms | 5.378 | 0.610 | 0.635 |
| warm labels: p95 ms | 7.307 | 1.284 | 1.302 |
| warm labels: peak RSS MiB | 6.996 | 5.652 | 5.605 |
| warm labels: CPU ms/12 | 58.285 | 9.041 | 9.301 |
| warm labels: open ms | 0.032 | 0.067 | 0.063 |
| warm labels: matches/12 | 60.000 | 60.000 | 60.000 |
| warm expand: p50 ms | 17.554 | 0.371 | 0.375 |
| warm expand: p95 ms | 18.316 | 0.468 | 0.460 |
| warm expand: peak RSS MiB | 7.316 | 5.555 | 5.543 |
| warm expand: CPU ms/12 | 211.798 | 4.983 | 5.070 |
| warm expand: open ms | 0.034 | 0.069 | 0.068 |
| warm expand: matches/12 | 60.000 | 60.000 | 60.000 |
| warm children: p50 ms | 16.771 | 0.657 | 0.661 |
| warm children: p95 ms | 17.404 | 0.989 | 0.992 |
| warm children: peak RSS MiB | 6.496 | 5.562 | 5.531 |
| warm children: CPU ms/12 | 202.790 | 8.464 | 8.440 |
| warm children: open ms | 0.033 | 0.068 | 0.065 |
| warm children: matches/12 | 1.000 | 1.000 | 1.000 |
| warm logs: p50 ms | 7.010 | 0.195 | 0.198 |
| warm logs: p95 ms | 7.487 | 0.294 | 0.272 |
| warm logs: peak RSS MiB | 5.746 | 5.578 | 5.602 |
| warm logs: CPU ms/12 | 85.873 | 3.318 | 3.569 |
| warm logs: open ms | 0.032 | 0.067 | 0.070 |
| warm logs: matches/12 | 192.000 | 192.000 | 192.000 |
| warm relations: p50 ms | 2.810 | 0.169 | 0.169 |
| warm relations: p95 ms | 3.434 | 0.199 | 0.203 |
| warm relations: peak RSS MiB | 6.082 | 5.605 | 5.605 |
| warm relations: CPU ms/12 | 35.366 | 2.884 | 3.001 |
| warm relations: open ms | 0.032 | 0.066 | 0.073 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 0.763 | 0.215 | 0.218 |
| warm id: p95 ms | 1.844 | 0.659 | 0.632 |
| warm id: peak RSS MiB | 6.086 | 5.547 | 5.598 |
| warm id: CPU ms/12 | 11.810 | 4.263 | 4.297 |
| warm id: open ms | 0.031 | 0.064 | 0.066 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 0.738 | 0.956 | 0.935 |
| warm clock: p95 ms | 0.923 | 1.036 | 0.982 |
| warm clock: peak RSS MiB | 5.375 | 5.586 | 5.512 |
| warm clock: CPU ms/12 | 10.211 | 12.385 | 12.144 |
| warm clock: open ms | 0.032 | 0.062 | 0.066 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |

### disordered

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 15.288 | 27.336 | 34.443 |
| write CPU ms | 16.100 | 28.200 | 38.495 |
| write max peak RSS MiB | 9.453 | 15.672 | 26.082 |
| read wall ms | 11.221 | 7.787 | 7.746 |
| read CPU ms | 12.270 | 8.790 | 8.758 |
| read max peak RSS MiB | 6.129 | 5.586 | 5.543 |
| null wall ms | 0.444 | 0.431 | 0.426 |
| null CPU ms | 1.461 | 1.415 | 1.294 |
| null max peak RSS MiB | 4.707 | 4.715 | 4.723 |
| write capture ms | 14.487 | 26.073 | 28.428 |
| write close ms | 0.699 | 1.060 | 5.779 |
| write file MiB | 0.413 | 0.493 | 0.493 |
| cold track: p50 ms | 5.634 | 0.097 | 0.097 |
| cold track: p95 ms | 6.589 | 0.145 | 0.140 |
| cold track: peak RSS MiB | 6.062 | 5.539 | 5.570 |
| cold track: CPU ms/12 | 69.815 | 2.224 | 2.206 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 40.000 | 40.000 | 40.000 |
| cold labels: p50 ms | 10.071 | 3.060 | 3.042 |
| cold labels: p95 ms | 14.283 | 3.426 | 3.456 |
| cold labels: peak RSS MiB | 7.277 | 5.574 | 5.594 |
| cold labels: CPU ms/12 | 128.766 | 31.976 | 31.377 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 80.000 | 80.000 | 80.000 |
| cold expand: p50 ms | 8.559 | 1.413 | 1.392 |
| cold expand: p95 ms | 9.307 | 1.553 | 1.493 |
| cold expand: peak RSS MiB | 7.352 | 5.488 | 5.559 |
| cold expand: CPU ms/12 | 103.515 | 15.862 | 15.795 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 60.000 | 60.000 | 60.000 |
| cold children: p50 ms | 7.437 | 1.208 | 1.231 |
| cold children: p95 ms | 8.422 | 1.959 | 1.985 |
| cold children: peak RSS MiB | 6.363 | 5.594 | 5.523 |
| cold children: CPU ms/12 | 91.682 | 14.906 | 14.632 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 2.000 | 2.000 | 2.000 |
| cold logs: p50 ms | 0.838 | 0.215 | 0.219 |
| cold logs: p95 ms | 0.981 | 0.289 | 0.316 |
| cold logs: peak RSS MiB | 5.176 | 5.590 | 5.539 |
| cold logs: CPU ms/12 | 11.391 | 3.151 | 3.436 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 12.000 | 12.000 | 12.000 |
| cold relations: p50 ms | 5.559 | 0.155 | 0.166 |
| cold relations: p95 ms | 6.540 | 0.281 | 0.281 |
| cold relations: peak RSS MiB | 6.125 | 5.551 | 5.566 |
| cold relations: CPU ms/12 | 68.529 | 2.848 | 2.750 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 0.996 | 1.162 | 1.180 |
| cold id: p95 ms | 1.898 | 1.713 | 1.714 |
| cold id: peak RSS MiB | 6.129 | 5.539 | 5.543 |
| cold id: CPU ms/12 | 12.933 | 12.363 | 12.166 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 1.494 | 1.875 | 1.859 |
| cold clock: p95 ms | 1.889 | 1.991 | 1.992 |
| cold clock: peak RSS MiB | 5.781 | 5.523 | 5.551 |
| cold clock: CPU ms/12 | 19.616 | 23.615 | 23.429 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 5.668 | 0.074 | 0.073 |
| warm track: p95 ms | 6.524 | 0.077 | 0.077 |
| warm track: peak RSS MiB | 6.105 | 5.539 | 5.559 |
| warm track: CPU ms/12 | 70.515 | 1.903 | 1.922 |
| warm track: open ms | 0.031 | 0.066 | 0.066 |
| warm track: matches/12 | 40.000 | 40.000 | 40.000 |
| warm labels: p50 ms | 10.011 | 3.009 | 2.999 |
| warm labels: p95 ms | 14.210 | 3.352 | 3.387 |
| warm labels: peak RSS MiB | 7.332 | 5.730 | 5.660 |
| warm labels: CPU ms/12 | 127.873 | 31.148 | 30.607 |
| warm labels: open ms | 0.032 | 0.061 | 0.055 |
| warm labels: matches/12 | 80.000 | 80.000 | 80.000 |
| warm expand: p50 ms | 8.443 | 1.371 | 1.374 |
| warm expand: p95 ms | 9.324 | 1.517 | 1.457 |
| warm expand: peak RSS MiB | 7.340 | 5.551 | 5.543 |
| warm expand: CPU ms/12 | 103.251 | 15.171 | 15.414 |
| warm expand: open ms | 0.035 | 0.066 | 0.064 |
| warm expand: matches/12 | 60.000 | 60.000 | 60.000 |
| warm children: p50 ms | 7.390 | 1.166 | 1.165 |
| warm children: p95 ms | 8.354 | 1.904 | 1.887 |
| warm children: peak RSS MiB | 6.402 | 5.535 | 5.488 |
| warm children: CPU ms/12 | 90.444 | 14.479 | 14.261 |
| warm children: open ms | 0.036 | 0.062 | 0.063 |
| warm children: matches/12 | 2.000 | 2.000 | 2.000 |
| warm logs: p50 ms | 0.831 | 0.191 | 0.190 |
| warm logs: p95 ms | 0.958 | 0.222 | 0.221 |
| warm logs: peak RSS MiB | 5.133 | 5.508 | 5.578 |
| warm logs: CPU ms/12 | 11.135 | 2.817 | 2.834 |
| warm logs: open ms | 0.035 | 0.061 | 0.064 |
| warm logs: matches/12 | 12.000 | 12.000 | 12.000 |
| warm relations: p50 ms | 5.709 | 0.130 | 0.128 |
| warm relations: p95 ms | 6.593 | 0.207 | 0.208 |
| warm relations: peak RSS MiB | 7.309 | 5.570 | 5.531 |
| warm relations: CPU ms/12 | 70.680 | 2.401 | 2.300 |
| warm relations: open ms | 0.032 | 0.058 | 0.060 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 0.999 | 1.140 | 1.152 |
| warm id: p95 ms | 1.851 | 1.679 | 1.663 |
| warm id: peak RSS MiB | 7.301 | 5.543 | 5.574 |
| warm id: CPU ms/12 | 13.529 | 12.070 | 11.979 |
| warm id: open ms | 0.035 | 0.061 | 0.058 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 1.463 | 1.822 | 1.824 |
| warm clock: p95 ms | 1.937 | 1.971 | 1.925 |
| warm clock: peak RSS MiB | 5.840 | 5.551 | 5.508 |
| warm clock: CPU ms/12 | 19.411 | 22.801 | 22.931 |
| warm clock: open ms | 0.034 | 0.061 | 0.063 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |

### large

| Metric | V2 | V3 sync | V3 workers=2 |
|---|---:|---:|---:|
| write wall ms | 78.676 | 47.527 | 35.542 |
| write CPU ms | 79.553 | 48.149 | 52.019 |
| write max peak RSS MiB | 29.004 | 14.848 | 20.680 |
| read wall ms | 60.028 | 53.860 | 54.032 |
| read CPU ms | 61.049 | 54.810 | 54.985 |
| read max peak RSS MiB | 12.840 | 5.629 | 5.648 |
| null wall ms | 0.176 | 0.179 | 0.173 |
| null CPU ms | 1.145 | 1.160 | 1.122 |
| null max peak RSS MiB | 4.715 | 4.738 | 4.723 |
| write capture ms | 76.772 | 46.671 | 33.101 |
| write close ms | 1.804 | 0.645 | 2.285 |
| write file MiB | 0.295 | 0.380 | 0.380 |
| cold track: p50 ms | 1.650 | 0.065 | 0.065 |
| cold track: p95 ms | 4.069 | 0.177 | 0.174 |
| cold track: peak RSS MiB | 8.895 | 5.672 | 5.633 |
| cold track: CPU ms/12 | 28.011 | 1.829 | 1.760 |
| cold track: open ms | 0.000 | 0.000 | 0.000 |
| cold track: matches/12 | 40.000 | 40.000 | 40.000 |
| cold labels: p50 ms | 7.846 | 0.975 | 0.970 |
| cold labels: p95 ms | 11.971 | 1.456 | 1.475 |
| cold labels: peak RSS MiB | 12.602 | 5.738 | 5.715 |
| cold labels: CPU ms/12 | 95.655 | 12.876 | 12.907 |
| cold labels: open ms | 0.000 | 0.000 | 0.000 |
| cold labels: matches/12 | 80.000 | 80.000 | 80.000 |
| cold expand: p50 ms | 15.398 | 0.706 | 0.711 |
| cold expand: p95 ms | 20.058 | 0.745 | 0.808 |
| cold expand: peak RSS MiB | 12.895 | 5.723 | 5.637 |
| cold expand: CPU ms/12 | 190.246 | 8.443 | 8.409 |
| cold expand: open ms | 0.000 | 0.000 | 0.000 |
| cold expand: matches/12 | 60.000 | 60.000 | 60.000 |
| cold children: p50 ms | 19.515 | 1.724 | 1.723 |
| cold children: p95 ms | 21.514 | 3.275 | 3.260 |
| cold children: peak RSS MiB | 12.594 | 5.668 | 5.688 |
| cold children: CPU ms/12 | 237.994 | 21.958 | 21.830 |
| cold children: open ms | 0.000 | 0.000 | 0.000 |
| cold children: matches/12 | 1.000 | 1.000 | 1.000 |
| cold logs: p50 ms | 2.751 | 0.066 | 0.065 |
| cold logs: p95 ms | 7.352 | 0.168 | 0.175 |
| cold logs: peak RSS MiB | 12.828 | 5.695 | 5.648 |
| cold logs: CPU ms/12 | 41.290 | 1.900 | 1.902 |
| cold logs: open ms | 0.000 | 0.000 | 0.000 |
| cold logs: matches/12 | 12.000 | 12.000 | 12.000 |
| cold relations: p50 ms | 11.414 | 0.050 | 0.049 |
| cold relations: p95 ms | 14.502 | 0.130 | 0.140 |
| cold relations: peak RSS MiB | 8.887 | 5.613 | 5.656 |
| cold relations: CPU ms/12 | 141.300 | 1.430 | 1.456 |
| cold relations: open ms | 0.000 | 0.000 | 0.000 |
| cold relations: matches/12 | 12.000 | 12.000 | 12.000 |
| cold id: p50 ms | 0.813 | 0.467 | 0.474 |
| cold id: p95 ms | 2.408 | 0.731 | 0.730 |
| cold id: peak RSS MiB | 8.875 | 5.652 | 5.668 |
| cold id: CPU ms/12 | 12.668 | 6.127 | 6.092 |
| cold id: open ms | 0.000 | 0.000 | 0.000 |
| cold id: matches/12 | 12.000 | 12.000 | 12.000 |
| cold clock: p50 ms | 0.499 | 3.407 | 3.411 |
| cold clock: p95 ms | 1.364 | 3.552 | 3.630 |
| cold clock: peak RSS MiB | 6.785 | 5.680 | 5.641 |
| cold clock: CPU ms/12 | 8.587 | 41.888 | 41.925 |
| cold clock: open ms | 0.000 | 0.000 | 0.000 |
| cold clock: matches/12 | 12.000 | 12.000 | 12.000 |
| warm track: p50 ms | 1.688 | 0.024 | 0.024 |
| warm track: p95 ms | 4.051 | 0.053 | 0.054 |
| warm track: peak RSS MiB | 8.914 | 5.668 | 5.672 |
| warm track: CPU ms/12 | 28.382 | 1.332 | 1.319 |
| warm track: open ms | 0.038 | 0.098 | 0.101 |
| warm track: matches/12 | 40.000 | 40.000 | 40.000 |
| warm labels: p50 ms | 7.888 | 0.891 | 0.933 |
| warm labels: p95 ms | 11.706 | 1.405 | 1.391 |
| warm labels: peak RSS MiB | 12.574 | 5.805 | 5.859 |
| warm labels: CPU ms/12 | 95.278 | 12.403 | 12.526 |
| warm labels: open ms | 0.045 | 0.099 | 0.100 |
| warm labels: matches/12 | 80.000 | 80.000 | 80.000 |
| warm expand: p50 ms | 15.487 | 0.663 | 0.661 |
| warm expand: p95 ms | 19.821 | 0.686 | 0.688 |
| warm expand: peak RSS MiB | 12.871 | 5.734 | 5.723 |
| warm expand: CPU ms/12 | 190.446 | 7.696 | 7.763 |
| warm expand: open ms | 0.044 | 0.106 | 0.108 |
| warm expand: matches/12 | 60.000 | 60.000 | 60.000 |
| warm children: p50 ms | 15.154 | 1.663 | 1.690 |
| warm children: p95 ms | 21.656 | 3.153 | 3.128 |
| warm children: peak RSS MiB | 12.969 | 5.652 | 5.598 |
| warm children: CPU ms/12 | 190.305 | 21.120 | 21.342 |
| warm children: open ms | 0.046 | 0.104 | 0.108 |
| warm children: matches/12 | 1.000 | 1.000 | 1.000 |
| warm logs: p50 ms | 2.671 | 0.030 | 0.030 |
| warm logs: p95 ms | 7.210 | 0.060 | 0.054 |
| warm logs: peak RSS MiB | 12.793 | 5.652 | 5.629 |
| warm logs: CPU ms/12 | 40.295 | 1.363 | 1.311 |
| warm logs: open ms | 0.047 | 0.109 | 0.099 |
| warm logs: matches/12 | 12.000 | 12.000 | 12.000 |
| warm relations: p50 ms | 11.376 | 0.015 | 0.015 |
| warm relations: p95 ms | 14.500 | 0.027 | 0.028 |
| warm relations: peak RSS MiB | 8.883 | 5.527 | 5.594 |
| warm relations: CPU ms/12 | 141.250 | 1.001 | 0.994 |
| warm relations: open ms | 0.048 | 0.097 | 0.096 |
| warm relations: matches/12 | 12.000 | 12.000 | 12.000 |
| warm id: p50 ms | 0.775 | 0.423 | 0.425 |
| warm id: p95 ms | 2.341 | 0.676 | 0.700 |
| warm id: peak RSS MiB | 8.836 | 5.711 | 5.758 |
| warm id: CPU ms/12 | 12.384 | 5.697 | 5.654 |
| warm id: open ms | 0.044 | 0.102 | 0.099 |
| warm id: matches/12 | 12.000 | 12.000 | 12.000 |
| warm clock: p50 ms | 0.474 | 3.359 | 3.338 |
| warm clock: p95 ms | 1.276 | 3.698 | 3.508 |
| warm clock: peak RSS MiB | 6.734 | 5.688 | 5.637 |
| warm clock: CPU ms/12 | 8.155 | 41.654 | 41.201 |
| warm clock: open ms | 0.049 | 0.110 | 0.103 |
| warm clock: matches/12 | 12.000 | 12.000 | 12.000 |
