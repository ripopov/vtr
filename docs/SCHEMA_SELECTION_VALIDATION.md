# Independent log schema validation

Measured 2026-10-26 on Intel Core Ultra 7 265K, Linux 7.0.0-38, glibc 2.43. All timed processes used logical CPU 0. Builds, static analysis, browser tests and simulator regressions finished before the final timing runs.

The fixed-source wire representation has no additional per-row selector. EVENT/STAGE segment headers remain 68 bytes; only LOG headers carry the extra 4-byte callsite reference. All six standard workloads and both additional fixed-record workloads have exactly equal baseline/candidate file sizes.

## Build provenance

- Unchanged VTR baseline: `48fc62525b1c0e9b5de0f8c445a08e729ab8e36f`, cloned under the ignored temporary directory. Its benchmark build records `vtr_dirty=false`; only generated reports and untracked measurement programs were added afterward.
- Native builds: GCC 15.2.0, C++20, `-O3 -DNDEBUG`, synchronous LZ4 writers, the same pinned FST/FTR dependencies and vendored container checksums. Both executables were built in temporary source checkouts with `python3 benchmarks/build.py`.
- The full default suite used three repetitions, scale divisor 1, all six profiles and all applicable formats/queries. Producer replays validate complete reads and every query.
- The paired suite used 21 repetitions in shuffled binary order, the same workload sizes, full reads and every applicable query. The fixed-record suite used 41 shuffled pairs per language.
- Additional Rust builds used the same Cargo release settings and lockfile, with rustc 1.101.0-nightly (c1070d693). The two temporary Rust programs have identical source; likewise for C++.

| Artifact | SHA-256 |
|---|---|
| Native baseline executable | `d4ea16bb91dc1f4637726415f26b9bf4a18936e8ada520d229e16125e0d19812` |
| Native candidate executable | `d06ae2f0f419473eaa058cee4ab6952c7137e06fc42811cc7476e78147763f0c` |
| Baseline tracked vtr-cpp files | `d4cb28d67e87aa9601f60ec23e679ccda7f6268dae28042b6892e8b5a8b064e8` |
| Candidate tracked vtr-cpp files | `898c396d72bfcc746ccf3d7c3819d36846d02b4de1624fc3283b6ef1bffad169` |
| Committed benchmark report | `9dae6484e138d3203e71c46cd7f04230d84b3cb9ed2f2fb0e214dd19dff03a03` |
| `baseline-default/raw.json` | `83479aa05c3e89145cc16259f40c7060ab40fd43634b199900e7c35699731a89` |
| `candidate-default/raw.json` | `9895a99969cb1aa7c5e54d8bad519a107367f8b6b2ae47f83c3fb6fceeb377f1` |
| `paired-final/raw.json` | `4af5644cc282db79abb2e96c1dd657f8fa12e70b5d8e9632dab7d6366155f1e1` |
| `fixed-paired.json` | `53b2ab962f933c56dd9f5f4b9432b0eb6e2dee03f1259aa7d31e241508e264ef` |
| `fixed_schema_bench.cpp` | `b7ff446de0b202277d21bf72383b6446fb4f659d3fb9a4fb1d1ea514801c388f` |
| `fixed_schema_bench.rs` | `af822a94ac46d0bc2a14f67a51ca92975b46eb8e211abfe0fc92460fcd52e1c9` |
| `query-pairs.json` | `e4521d6f91334bca3a7e787b5d505bcc539354c92ce825a8b679d6e17d286150` |

Source fingerprints hash sorted tracked paths, a NUL separator and each file’s binary SHA-256 digest. Raw measurements, temporary sources, build logs and dependency installations remain untracked under `atemp/schema-final` in the parent Volna worktree.

## Fixed-source timing

The acceptance limit is a 5% slowdown in fixed-source read/write time. All six standard workloads and both additional fixed-record workloads pass; encoded sizes are identical.

Changes are candidate/baseline − 1; lower time is better. The interval resamples whole shuffled pairs 10,000 times and reports the 95% bootstrap interval for the median paired ratio. It describes this host/run, not portability or an asynchronous scaling result.

| Workload | Write ms, baseline → candidate | Write change | Read ms, baseline → candidate | Read change | File bytes, both |
|---|---:|---:|---:|---:|---:|
| core | 104.068 → 104.249 | +0.17%; paired 95% [+0.23, +0.80]% | 37.153 → 37.180 | +0.07%; paired 95% [-0.38, +0.46]% | 8068791 |
| soc | 380.001 → 381.154 | +0.30%; paired 95% [-0.04, +0.61]% | 115.967 → 115.816 | -0.13%; paired 95% [-0.41, +0.06]% | 25772852 |
| idle | 52.790 → 53.060 | +0.51%; paired 95% [-0.23, +0.90]% | 16.009 → 16.003 | -0.04%; paired 95% [-0.68, +0.20]% | 1816477 |
| requests | 27.253 → 27.122 | -0.48%; paired 95% [-1.39, +0.38]% | 20.840 → 19.964 | -4.21%; paired 95% [-5.33, -3.24]% | 1096917 |
| many-generators | 49.523 → 49.591 | +0.14%; paired 95% [-0.77, +0.87]% | 35.161 → 33.483 | -4.77%; paired 95% [-5.57, -3.70]% | 1742740 |
| mixed | 34.686 → 34.676 | -0.03%; paired 95% [-0.60, +0.29]% | 16.030 → 15.612 | -2.60%; paired 95% [-3.47, -1.12]% | 2065357 |
| C++ fixed records | 15.531 → 15.539 | +0.05%; paired 95% [-0.56, +0.21]% | 12.829 → 11.944 | -6.90%; paired 95% [-7.32, -6.37]% | 352865 |
| Rust fixed records | 68.872 → 68.623 | -0.36%; paired 95% [-0.63, +0.27]% | 35.001 → 34.602 | -1.14%; paired 95% [-1.93, -0.53]% | 19408657 |

The additional workload writes 60,000 ordinary owners, one fixed u64 stage and one fixed u64 event per owner, plus one attributes-only embedded event: 240,000 records. Reads verify IDs, owners, timestamps, intervals, payloads, embedded detail and a digest; full validation runs outside the timed read. Each language keeps its own unchanged default encoding options, so cross-language byte counts are not a format-efficiency comparison.

Earlier runs under concurrent static-analysis load were inconclusive and were repeated without those jobs. A quiet fixed-record run exposed a 3.50% Rust read slowdown and a 1.51% C++ slowdown. Retained records now share immutable recording identity, datatypes and allocation limits; C++ uses one typed annotation field for mutually exclusive embedded names and log callsites, and places record metadata and payload storage in one allocation. Native payload views retain that immutable record, including its attributes and embedded detail, until the last view is dropped. Rust retention factories are inline, and native retention resolves a generator’s kind and datatype together. Transaction decoding reuses the resolved generator for payload decoding and validation, with an inline cached lookup. Fixed-source queue cells and located-record buffer footprints remain unchanged.

The many-generators full-trace query was investigated separately. Its pooled p50 is 4.93% slower and pooled p95 is 7.97% slower in the 21-pair suite. The median of paired per-run p95 ratios is +4.63% (95% bootstrap interval +3.80% to +7.74%). A separate 31-pair repeat with the same final executables, CPU and independent traces gives p50 11.269 → 11.700 ms (+3.82%) and p95 13.404 → 13.836 ms (+3.22%); all twelve producer digests match on every run. The original slower tail remains in the tables below. These fresh-reader queries include opening the recording and constructing schema/catalogue metadata, so their cost is distinct from sequential read throughput.

## Every paired native metric

### core

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 104.07 | 104.25 | +0.2% |
| Write throughput (Mobs/s) | 60.452 | 60.347 | -0.2% |
| File size (MiB) | 7.695 | 7.695 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.86 | 11.86 | +0.0% |
| Write peak RSS (MiB) | 18.86 | 19.02 | +0.8% |
| Capture time (ms) | 102.79 | 102.96 | +0.2% |
| Capture time (ns/tick) | 214.2 | 214.5 | +0.1% |
| File bytes per tick | 16.8 | 16.8 | +0.0% |
| Close time (ms) | 1.14 | 1.14 | +0.0% |
| Write CPU time (ms) | 104.60 | 104.78 | +0.2% |
| Write min / max (ms) | 102.90 / 108.10 | 103.81 / 109.71 | — |
| Read time (ms) | 37.15 | 37.18 | +0.1% |
| Read throughput (Mobs/s) | 169.328 | 169.208 | -0.1% |
| Read peak RSS (MiB) | 14.03 | 14.18 | +1.1% |
| search-signal: probes | 252 | 252 | +0.0% |
| search-signal: matches | 2,037 | 2,037 | +0.0% |
| search-signal: p50 (ms) | 0.109 | 0.109 | +0.0% |
| search-signal: p95 (ms) | 0.351 | 0.348 | -0.9% |
| search-signal: peak RSS (MiB) | 7.75 | 7.82 | +0.9% |
| find-signal: probes | 252 | 252 | +0.0% |
| find-signal: matches | 7,480,389 | 7,480,389 | +0.0% |
| find-signal: p50 (ms) | 0.611 | 0.609 | -0.3% |
| find-signal: p95 (ms) | 2.591 | 2.601 | +0.4% |
| find-signal: peak RSS (MiB) | 10.29 | 10.36 | +0.7% |
| point: probes | 252 | 252 | +0.0% |
| point: matches | 252 | 252 | +0.0% |
| point: p50 (ms) | 0.115 | 0.115 | +0.0% |
| point: p95 (ms) | 0.402 | 0.399 | -0.7% |
| point: peak RSS (MiB) | 7.81 | 7.88 | +0.9% |

### soc

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 380.00 | 381.15 | +0.3% |
| Write throughput (Mobs/s) | 53.010 | 52.849 | -0.3% |
| File size (MiB) | 24.579 | 24.579 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.89 | 11.89 | +0.0% |
| Write peak RSS (MiB) | 16.19 | 16.18 | -0.1% |
| Capture time (ms) | 373.04 | 374.11 | +0.3% |
| Capture time (ns/tick) | 15543.2 | 15587.9 | +0.3% |
| File bytes per tick | 1073.9 | 1073.9 | +0.0% |
| Close time (ms) | 5.89 | 5.92 | +0.5% |
| Write CPU time (ms) | 380.51 | 381.68 | +0.3% |
| Write min / max (ms) | 377.99 / 383.20 | 378.30 / 397.55 | — |
| Read time (ms) | 115.97 | 115.82 | -0.1% |
| Read throughput (Mobs/s) | 173.703 | 173.929 | +0.1% |
| Read peak RSS (MiB) | 30.71 | 30.77 | +0.2% |
| search-signal: probes | 252 | 252 | +0.0% |
| search-signal: matches | 2,079 | 2,079 | +0.0% |
| search-signal: p50 (ms) | 0.407 | 0.404 | -0.7% |
| search-signal: p95 (ms) | 0.561 | 0.562 | +0.2% |
| search-signal: peak RSS (MiB) | 7.13 | 7.17 | +0.6% |
| find-signal: probes | 252 | 252 | +0.0% |
| find-signal: matches | 374,619 | 374,619 | +0.0% |
| find-signal: p50 (ms) | 0.530 | 0.532 | +0.4% |
| find-signal: p95 (ms) | 0.691 | 0.694 | +0.4% |
| find-signal: peak RSS (MiB) | 9.45 | 9.57 | +1.3% |
| point: probes | 252 | 252 | +0.0% |
| point: matches | 252 | 252 | +0.0% |
| point: p50 (ms) | 0.403 | 0.402 | -0.2% |
| point: p95 (ms) | 0.541 | 0.534 | -1.3% |
| point: peak RSS (MiB) | 7.10 | 7.21 | +1.5% |

### idle

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 52.79 | 53.06 | +0.5% |
| Write throughput (Mobs/s) | 48.249 | 48.004 | -0.5% |
| File size (MiB) | 1.732 | 1.732 | +0.0% |
| Bytes per observation | 0.71 | 0.71 | +0.0% |
| Logical bytes / file bytes | 21.34 | 21.34 | +0.0% |
| Write peak RSS (MiB) | 15.36 | 15.39 | +0.2% |
| Capture time (ms) | 45.16 | 45.43 | +0.6% |
| Capture time (ns/tick) | 940.8 | 946.6 | +0.6% |
| File bytes per tick | 37.8 | 37.8 | +0.0% |
| Close time (ms) | 6.66 | 6.65 | -0.2% |
| Write CPU time (ms) | 53.31 | 53.56 | +0.5% |
| Write min / max (ms) | 52.48 / 54.45 | 52.57 / 60.97 | — |
| Read time (ms) | 16.01 | 16.00 | -0.1% |
| Read throughput (Mobs/s) | 159.102 | 159.165 | +0.0% |
| Read peak RSS (MiB) | 7.95 | 8.10 | +1.9% |
| search-signal: probes | 252 | 252 | +0.0% |
| search-signal: matches | 21 | 21 | +0.0% |
| search-signal: p50 (ms) | 0.377 | 0.380 | +0.8% |
| search-signal: p95 (ms) | 0.569 | 0.567 | -0.4% |
| search-signal: peak RSS (MiB) | 6.75 | 6.80 | +0.7% |
| find-signal: probes | 252 | 252 | +0.0% |
| find-signal: matches | 47,208 | 47,208 | +0.0% |
| find-signal: p50 (ms) | 0.397 | 0.397 | +0.0% |
| find-signal: p95 (ms) | 0.619 | 0.624 | +0.8% |
| find-signal: peak RSS (MiB) | 6.77 | 6.88 | +1.6% |
| point: probes | 252 | 252 | +0.0% |
| point: matches | 252 | 252 | +0.0% |
| point: p50 (ms) | 0.383 | 0.391 | +2.1% |
| point: p95 (ms) | 0.564 | 0.562 | -0.4% |
| point: peak RSS (MiB) | 6.82 | 6.84 | +0.3% |

### requests

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 27.25 | 27.12 | -0.5% |
| Write throughput (Mobs/s) | 7.549 | 7.585 | +0.5% |
| File size (MiB) | 1.046 | 1.046 | +0.0% |
| Bytes per observation | 5.33 | 5.33 | +0.0% |
| Logical bytes / file bytes | 12.75 | 12.75 | +0.0% |
| Write peak RSS (MiB) | 6.52 | 6.52 | +0.0% |
| Capture time (ms) | 27.01 | 26.87 | -0.5% |
| Capture time (ns/tick) | 225.1 | 223.9 | -0.5% |
| File bytes per tick | 9.1 | 9.1 | +0.0% |
| Close time (ms) | 0.10 | 0.10 | +0.0% |
| Write CPU time (ms) | 27.82 | 27.75 | -0.3% |
| Write min / max (ms) | 26.90 / 28.33 | 26.83 / 27.95 | — |
| Read time (ms) | 20.84 | 19.96 | -4.2% |
| Read throughput (Mobs/s) | 9.872 | 10.305 | +4.4% |
| Read peak RSS (MiB) | 6.37 | 6.37 | +0.0% |
| search-transaction: probes | 252 | 252 | +0.0% |
| search-transaction: matches | 189 | 189 | +0.0% |
| search-transaction: p50 (ms) | 0.105 | 0.106 | +1.0% |
| search-transaction: p95 (ms) | 0.158 | 0.154 | -2.5% |
| search-transaction: peak RSS (MiB) | 6.31 | 6.35 | +0.6% |
| find-transaction: probes | 252 | 252 | +0.0% |
| find-transaction: matches | 19,005 | 19,005 | +0.0% |
| find-transaction: p50 (ms) | 8.477 | 8.662 | +2.2% |
| find-transaction: p95 (ms) | 9.982 | 10.311 | +3.3% |
| find-transaction: peak RSS (MiB) | 6.32 | 6.41 | +1.4% |

### many-generators

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 49.52 | 49.59 | +0.1% |
| Write throughput (Mobs/s) | 6.647 | 6.638 | -0.1% |
| File size (MiB) | 1.662 | 1.662 | +0.0% |
| Bytes per observation | 5.29 | 5.29 | +0.0% |
| Logical bytes / file bytes | 12.84 | 12.84 | +0.0% |
| Write peak RSS (MiB) | 6.66 | 6.69 | +0.5% |
| Capture time (ms) | 49.26 | 49.35 | +0.2% |
| Capture time (ns/tick) | 2052.6 | 2056.3 | +0.2% |
| File bytes per tick | 72.6 | 72.6 | +0.0% |
| Close time (ms) | 0.11 | 0.11 | +0.0% |
| Write CPU time (ms) | 50.09 | 50.15 | +0.1% |
| Write min / max (ms) | 49.00 / 50.21 | 48.93 / 58.44 | — |
| Read time (ms) | 35.16 | 33.48 | -4.8% |
| Read throughput (Mobs/s) | 9.362 | 9.831 | +5.0% |
| Read peak RSS (MiB) | 7.07 | 7.12 | +0.7% |
| search-transaction: probes | 252 | 252 | +0.0% |
| search-transaction: matches | 105 | 105 | +0.0% |
| search-transaction: p50 (ms) | 0.282 | 0.279 | -1.1% |
| search-transaction: p95 (ms) | 0.359 | 0.346 | -3.6% |
| search-transaction: peak RSS (MiB) | 7.12 | 7.20 | +1.1% |
| find-transaction: probes | 252 | 252 | +0.0% |
| find-transaction: matches | 3,759 | 3,759 | +0.0% |
| find-transaction: p50 (ms) | 11.377 | 11.938 | +4.9% |
| find-transaction: p95 (ms) | 13.245 | 14.300 | +8.0% |
| find-transaction: peak RSS (MiB) | 7.18 | 7.21 | +0.4% |

### mixed

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 34.69 | 34.68 | -0.0% |
| Write throughput (Mobs/s) | 38.669 | 38.680 | +0.0% |
| File size (MiB) | 1.970 | 1.970 | +0.0% |
| Bytes per observation | 1.54 | 1.54 | +0.0% |
| Logical bytes / file bytes | 11.98 | 11.98 | +0.0% |
| Write peak RSS (MiB) | 15.73 | 15.77 | +0.3% |
| Capture time (ms) | 30.89 | 30.92 | +0.1% |
| Capture time (ns/tick) | 1287.0 | 1288.5 | +0.1% |
| File bytes per tick | 86.1 | 86.1 | +0.0% |
| Close time (ms) | 3.59 | 3.61 | +0.6% |
| Write CPU time (ms) | 35.22 | 35.23 | +0.0% |
| Write min / max (ms) | 34.44 / 35.44 | 34.38 / 35.80 | — |
| Read time (ms) | 16.03 | 15.61 | -2.6% |
| Read throughput (Mobs/s) | 83.675 | 85.912 | +2.7% |
| Read peak RSS (MiB) | 7.45 | 7.45 | +0.0% |
| search-signal: probes | 252 | 252 | +0.0% |
| search-signal: matches | 2,058 | 2,058 | +0.0% |
| search-signal: p50 (ms) | 0.079 | 0.079 | +0.0% |
| search-signal: p95 (ms) | 0.204 | 0.206 | +1.0% |
| search-signal: peak RSS (MiB) | 6.68 | 6.78 | +1.5% |
| find-signal: probes | 252 | 252 | +0.0% |
| find-signal: matches | 374,472 | 374,472 | +0.0% |
| find-signal: p50 (ms) | 0.094 | 0.097 | +3.2% |
| find-signal: p95 (ms) | 0.227 | 0.227 | +0.0% |
| find-signal: peak RSS (MiB) | 6.70 | 6.80 | +1.5% |
| point: probes | 252 | 252 | +0.0% |
| point: matches | 252 | 252 | +0.0% |
| point: p50 (ms) | 0.082 | 0.083 | +1.2% |
| point: p95 (ms) | 0.205 | 0.205 | +0.0% |
| point: peak RSS (MiB) | 6.73 | 6.81 | +1.2% |
| search-transaction: probes | 252 | 252 | +0.0% |
| search-transaction: matches | 168 | 168 | +0.0% |
| search-transaction: p50 (ms) | 0.140 | 0.141 | +0.7% |
| search-transaction: p95 (ms) | 0.199 | 0.194 | -2.5% |
| search-transaction: peak RSS (MiB) | 5.85 | 5.95 | +1.7% |
| find-transaction: probes | 252 | 252 | +0.0% |
| find-transaction: matches | 3,801 | 3,801 | +0.0% |
| find-transaction: p50 (ms) | 3.595 | 3.662 | +1.9% |
| find-transaction: p95 (ms) | 4.578 | 4.721 | +3.1% |
| find-transaction: peak RSS (MiB) | 5.94 | 5.95 | +0.2% |

## Default suite compared with the committed report

Every cell below is **committed → freshly built unchanged baseline → candidate**. This includes other formats as controls, producer costs, RSS, every query latency and every measured count. The complete candidate report is [benchmarks/report.md](../benchmarks/report.md). Changes shared by the unchanged baseline or external formats are environment/run variation, rather than evidence of a schema-selection cost. Slower results are retained in these tables.

### core

Observations: 6,291,079. Null producer: 47.72 ms.
Observations: 6,291,079. Null producer: 46.20 ms.
Observations: 6,291,079. Null producer: 48.33 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 108.08 → 106.28 → 113.34 | 557.47 → 568.81 → 652.49 | 158.57 → 155.55 → 164.56 |
| Write throughput (Mobs/s) | 58.208 → 59.196 → 55.505 | 11.285 → 11.060 → 9.642 | 39.675 → 40.445 → 38.231 |
| Read time (ms) | 37.56 → 37.17 → 39.23 | 166.13 → 163.45 → 168.39 | 176.30 → 180.16 → 185.12 |
| Read throughput (Mobs/s) | 167.487 → 169.262 → 160.348 | 37.869 → 38.488 → 37.359 | 35.684 → 34.919 → 33.985 |
| File size (MiB) | 7.695 → 7.695 → 7.695 | 8.941 → 8.941 → 8.941 | 149.324 → 149.324 → 149.324 |
| Bytes per observation | 1.28 → 1.28 → 1.28 | 1.49 → 1.49 → 1.49 | 24.89 → 24.89 → 24.89 |
| Logical bytes / file bytes | 11.86 → 11.86 → 11.86 | 10.21 → 10.21 → 10.21 | 0.61 → 0.61 → 0.61 |
| Write peak RSS (MiB) | 19.11 → 18.90 → 19.03 | 140.96 → 141.08 → 141.05 | 5.43 → 5.47 → 5.60 |
| Read peak RSS (MiB) | 14.06 → 14.11 → 14.24 | 26.22 → 26.35 → 26.24 | 5.22 → 5.35 → 5.23 |
| Capture time (ms) | 106.65 → 104.84 → 111.86 | 486.93 → 497.17 → 566.76 | 158.07 → 154.92 → 163.94 |
| Capture time (ns/tick) | 222.2 → 218.4 → 233.0 | 1014.4 → 1035.8 → 1180.7 | 329.3 → 322.8 → 341.5 |
| File bytes per tick | 16.8 → 16.8 → 16.8 | 19.5 → 19.5 → 19.5 | 326.2 → 326.2 → 326.2 |
| Close time (ms) | 1.27 → 1.28 → 1.47 | 70.42 → 71.20 → 92.58 | 0.11 → 0.10 → 0.09 |
| Write CPU time (ms) | 108.64 → 107.01 → 114.13 | 558.18 → 569.57 → 653.35 | 159.07 → 156.32 → 165.34 |
| Write min / max (ms) | 106.22 / 109.86 → 104.23 / 107.29 → 111.50 / 113.53 | 546.42 / 604.96 → 563.57 / 569.58 → 651.26 / 769.14 | 157.77 / 159.66 → 153.41 / 157.08 → 158.10 / 165.53 |
| search-signal: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| search-signal: matches | 291 → 291 → 291 | 291 → 291 → 291 | 291 → 291 → 291 |
| search-signal: p50 (ms) | 0.109 → 0.110 → 0.116 | 2.337 → 2.283 → 2.446 | 54.779 → 56.402 → 55.703 |
| search-signal: p95 (ms) | 0.402 → 0.377 → 0.405 | 6.006 → 5.970 → 6.323 | 87.740 → 88.276 → 89.818 |
| search-signal: peak RSS (MiB) | 7.67 → 7.88 → 7.88 | 26.91 → 26.96 → 26.98 | 6.64 → 6.64 → 6.68 |
| find-signal: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| find-signal: matches | 1,068,627 → 1,068,627 → 1,068,627 | 1,068,627 → 1,068,627 → 1,068,627 | 1,068,627 → 1,068,627 → 1,068,627 |
| find-signal: p50 (ms) | 0.616 → 0.611 → 0.644 | 2.789 → 2.718 → 2.957 | 91.981 → 91.519 → 95.998 |
| find-signal: p95 (ms) | 2.637 → 2.606 → 2.721 | 8.216 → 8.035 → 8.656 | 95.486 → 95.643 → 134.624 |
| find-signal: peak RSS (MiB) | 10.26 → 10.32 → 10.33 | 26.94 → 26.98 → 27.00 | 6.62 → 6.71 → 6.61 |
| point: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| point: matches | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| point: p50 (ms) | 0.117 → 0.116 → 0.119 | 0.349 → 0.354 → 0.391 | 55.065 → 55.414 → 58.095 |
| point: p95 (ms) | 0.431 → 0.425 → 0.454 | 1.211 → 1.163 → 1.406 | 89.248 → 89.027 → 90.589 |
| point: peak RSS (MiB) | 7.72 → 7.75 → 7.75 | 8.92 → 8.93 → 8.97 | 6.65 → 6.68 → 6.73 |

### soc

Observations: 20,143,793. Null producer: 136.84 ms.
Observations: 20,143,793. Null producer: 135.49 ms.
Observations: 20,143,793. Null producer: 136.98 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 391.23 → 379.86 → 383.71 | 806.21 → 799.58 → 802.29 | 486.40 → 474.97 → 477.44 |
| Write throughput (Mobs/s) | 51.488 → 53.029 → 52.498 | 24.986 → 25.193 → 25.108 | 41.414 → 42.410 → 42.192 |
| Read time (ms) | 116.80 → 115.85 → 116.04 | 575.06 → 571.01 → 568.11 | 549.58 → 555.15 → 551.22 |
| Read throughput (Mobs/s) | 172.458 → 173.873 → 173.587 | 35.029 → 35.278 → 35.457 | 36.653 → 36.286 → 36.544 |
| File size (MiB) | 24.579 → 24.579 → 24.579 | 52.345 → 52.345 → 52.345 | 485.555 → 485.555 → 485.555 |
| Bytes per observation | 1.28 → 1.28 → 1.28 | 2.72 → 2.72 → 2.72 | 25.28 → 25.28 → 25.28 |
| Logical bytes / file bytes | 11.89 → 11.89 → 11.89 | 5.58 → 5.58 → 5.58 | 0.60 → 0.60 → 0.60 |
| Write peak RSS (MiB) | 16.03 → 16.04 → 16.16 | 145.49 → 145.51 → 145.58 | 5.60 → 5.74 → 5.74 |
| Read peak RSS (MiB) | 30.71 → 30.71 → 30.75 | 22.63 → 22.65 → 22.64 | 5.25 → 5.09 → 5.13 |
| Capture time (ms) | 384.09 → 372.95 → 376.80 | 782.82 → 776.11 → 778.58 | 485.30 → 474.03 → 476.53 |
| Capture time (ns/tick) | 16003.7 → 15539.6 → 15700.0 | 32617.7 → 32337.9 → 32441.0 | 20220.9 → 19751.5 → 19855.5 |
| File bytes per tick | 1073.9 → 1073.9 → 1073.9 | 2287.0 → 2287.0 → 2287.0 | 21214.2 → 21214.2 → 21214.2 |
| Close time (ms) | 6.12 → 5.78 → 5.76 | 23.09 → 22.79 → 23.17 | 0.39 → 0.15 → 0.14 |
| Write CPU time (ms) | 391.90 → 380.72 → 384.48 | 806.65 → 799.96 → 801.92 | 486.55 → 475.29 → 477.90 |
| Write min / max (ms) | 390.84 / 403.17 → 378.03 / 387.57 → 382.99 / 384.56 | 801.17 / 809.12 → 798.07 / 801.38 → 798.82 / 816.37 | 477.95 / 498.15 → 473.00 / 476.51 → 474.92 / 479.44 |
| search-signal: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| search-signal: matches | 297 → 297 → 297 | 297 → 297 → 297 | 297 → 297 → 297 |
| search-signal: p50 (ms) | 0.412 → 0.406 → 0.401 | 0.394 → 0.399 → 0.393 | 177.766 → 176.670 → 176.553 |
| search-signal: p95 (ms) | 0.601 → 0.581 → 0.578 | 6.397 → 6.026 → 5.757 | 252.141 → 251.399 → 249.252 |
| search-signal: peak RSS (MiB) | 7.04 → 7.09 → 7.00 | 22.67 → 22.73 → 22.70 | 6.66 → 6.68 → 6.64 |
| find-signal: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| find-signal: matches | 53,517 → 53,517 → 53,517 | 53,517 → 53,517 → 53,517 | 53,517 → 53,517 → 53,517 |
| find-signal: p50 (ms) | 0.545 → 0.527 → 0.528 | 6.930 → 6.665 → 6.498 | 270.434 → 269.069 → 270.224 |
| find-signal: p95 (ms) | 0.727 → 0.702 → 0.695 | 7.805 → 7.415 → 6.889 | 275.378 → 278.177 → 275.962 |
| find-signal: peak RSS (MiB) | 9.46 → 9.53 → 9.43 | 22.38 → 22.30 → 22.43 | 6.62 → 6.67 → 6.69 |
| point: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| point: matches | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| point: p50 (ms) | 0.411 → 0.406 → 0.401 | 0.190 → 0.198 → 0.197 | 178.284 → 176.368 → 175.359 |
| point: p95 (ms) | 0.565 → 0.545 → 0.538 | 0.282 → 0.306 → 0.271 | 251.476 → 250.030 → 249.426 |
| point: peak RSS (MiB) | 6.96 → 7.11 → 7.07 | 5.03 → 5.09 → 5.11 | 6.66 → 6.69 → 6.64 |

### idle

Observations: 2,547,072. Null producer: 17.20 ms.
Observations: 2,547,072. Null producer: 17.49 ms.
Observations: 2,547,072. Null producer: 17.33 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 53.75 → 53.43 → 53.16 | 109.57 → 108.77 → 107.98 | 61.57 → 60.73 → 61.15 |
| Write throughput (Mobs/s) | 47.391 → 47.667 → 47.918 | 23.245 → 23.417 → 23.588 | 41.366 → 41.942 → 41.650 |
| Read time (ms) | 16.12 → 16.23 → 15.97 | 71.36 → 72.22 → 71.05 | 69.77 → 74.29 → 70.45 |
| Read throughput (Mobs/s) | 157.961 → 156.923 → 159.541 | 35.691 → 35.266 → 35.848 | 36.507 → 34.287 → 36.152 |
| File size (MiB) | 1.732 → 1.732 → 1.732 | 4.165 → 4.165 → 4.165 | 61.551 → 61.551 → 61.551 |
| Bytes per observation | 0.71 → 0.71 → 0.71 | 1.71 → 1.71 → 1.71 | 25.34 → 25.34 → 25.34 |
| Logical bytes / file bytes | 21.34 → 21.34 → 21.34 | 8.88 → 8.88 → 8.88 | 0.60 → 0.60 → 0.60 |
| Write peak RSS (MiB) | 15.15 → 15.47 → 15.58 | 77.49 → 77.69 → 77.67 | 5.62 → 5.77 → 5.74 |
| Read peak RSS (MiB) | 7.95 → 7.97 → 7.95 | 14.62 → 14.74 → 14.66 | 5.23 → 5.38 → 5.23 |
| Capture time (ms) | 45.84 → 45.60 → 45.39 | 55.99 → 55.40 → 54.73 | 60.71 → 59.90 → 60.42 |
| Capture time (ns/tick) | 954.9 → 950.0 → 945.7 | 1166.4 → 1154.1 → 1140.2 | 1264.8 → 1247.9 → 1258.7 |
| File bytes per tick | 37.8 → 37.8 → 37.8 | 91.0 → 91.0 → 91.0 | 1344.6 → 1344.6 → 1344.6 |
| Close time (ms) | 6.72 → 6.76 → 6.67 | 53.08 → 52.95 → 52.85 | 0.14 → 0.14 → 0.12 |
| Write CPU time (ms) | 54.58 → 54.19 → 53.73 | 110.24 → 109.30 → 108.49 | 62.13 → 61.29 → 61.68 |
| Write min / max (ms) | 53.70 / 54.01 → 53.27 / 55.72 → 52.91 / 54.78 | 109.44 / 109.79 → 108.54 / 114.76 → 107.91 / 108.59 | 61.25 / 62.26 → 60.55 / 64.07 → 60.88 / 61.24 |
| search-signal: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| search-signal: matches | 3 → 3 → 3 | 3 → 3 → 3 | 3 → 3 → 3 |
| search-signal: p50 (ms) | 0.380 → 0.385 → 0.378 | 0.311 → 0.321 → 0.329 | 12.848 → 13.723 → 13.391 |
| search-signal: p95 (ms) | 0.608 → 0.581 → 0.577 | 3.743 → 3.593 → 3.601 | 32.014 → 33.366 → 33.384 |
| search-signal: peak RSS (MiB) | 6.74 → 6.80 → 6.91 | 15.09 → 15.07 → 15.12 | 6.66 → 6.68 → 6.71 |
| find-signal: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| find-signal: matches | 6,744 → 6,744 → 6,744 | 6,744 → 6,744 → 6,744 | 6,744 → 6,744 → 6,744 |
| find-signal: p50 (ms) | 0.397 → 0.403 → 0.396 | 0.315 → 0.317 → 0.315 | 33.223 → 34.506 → 34.485 |
| find-signal: p95 (ms) | 0.633 → 0.625 → 0.628 | 3.452 → 3.394 → 3.324 | 34.240 → 36.474 → 35.289 |
| find-signal: peak RSS (MiB) | 6.77 → 6.72 → 6.73 | 15.06 → 15.11 → 15.09 | 6.64 → 6.66 → 6.69 |
| point: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| point: matches | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| point: p50 (ms) | 0.396 → 0.408 → 0.392 | 0.091 → 0.093 → 0.093 | 12.879 → 13.374 → 13.136 |
| point: p95 (ms) | 0.587 → 0.570 → 0.569 | 0.259 → 0.268 → 0.255 | 32.352 → 33.664 → 33.229 |
| point: peak RSS (MiB) | 6.78 → 6.79 → 6.73 | 5.48 → 5.50 → 5.47 | 6.67 → 6.68 → 6.66 |

### requests

Observations: 205,728. Null producer: 7.07 ms.
Observations: 205,728. Null producer: 7.00 ms.
Observations: 205,728. Null producer: 6.96 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 28.58 → 27.41 → 27.38 | 36.89 → 35.90 → 35.95 |
| Write throughput (Mobs/s) | 7.198 → 7.505 → 7.514 | 5.576 → 5.730 → 5.723 |
| Read time (ms) | 21.76 → 20.89 → 20.36 | 10.83 → 10.84 → 10.77 |
| Read throughput (Mobs/s) | 9.454 → 9.849 → 10.106 | 19.002 → 18.979 → 19.102 |
| File size (MiB) | 1.046 → 1.046 → 1.046 | 4.424 → 4.424 → 4.424 |
| Bytes per observation | 5.33 → 5.33 → 5.33 | 22.55 → 22.55 → 22.55 |
| Logical bytes / file bytes | 12.75 → 12.75 → 12.75 | 3.02 → 3.02 → 3.02 |
| Write peak RSS (MiB) | 6.38 → 6.40 → 6.42 | 5.00 → 5.00 → 4.96 |
| Read peak RSS (MiB) | 6.16 → 6.36 → 6.34 | 8.78 → 8.76 → 8.84 |
| Capture time (ms) | 28.30 → 27.15 → 27.11 | 36.71 → 35.73 → 35.79 |
| Capture time (ns/tick) | 235.8 → 226.3 → 225.9 | 305.9 → 297.7 → 298.2 |
| File bytes per tick | 9.1 → 9.1 → 9.1 | 38.7 → 38.7 → 38.7 |
| Close time (ms) | 0.11 → 0.11 → 0.10 | 0.14 → 0.14 → 0.13 |
| Write CPU time (ms) | 29.27 → 27.95 → 28.07 | 37.44 → 36.45 → 36.50 |
| Write min / max (ms) | 27.64 / 29.67 → 27.15 / 27.50 → 27.11 / 28.11 | 36.12 / 36.96 → 35.57 / 36.11 → 35.44 / 36.20 |
| search-transaction: probes | 36 → 36 → 36 | 36 → 36 → 36 |
| search-transaction: matches | 27 → 27 → 27 | 27 → 27 → 27 |
| search-transaction: p50 (ms) | 0.111 → 0.106 → 0.107 | 3.285 → 3.253 → 3.222 |
| search-transaction: p95 (ms) | 0.176 → 0.163 → 0.156 | 4.180 → 4.128 → 4.129 |
| search-transaction: peak RSS (MiB) | 6.18 → 6.34 → 6.24 | 8.74 → 8.79 → 8.83 |
| find-transaction: probes | 36 → 36 → 36 | 36 → 36 → 36 |
| find-transaction: matches | 2,715 → 2,715 → 2,715 | 2,715 → 2,715 → 2,715 |
| find-transaction: p50 (ms) | 8.690 → 8.523 → 8.748 | 4.653 → 4.637 → 4.630 |
| find-transaction: p95 (ms) | 9.566 → 9.963 → 10.127 | 4.813 → 4.922 → 4.705 |
| find-transaction: peak RSS (MiB) | 6.18 → 6.36 → 6.27 | 8.78 → 8.75 → 8.84 |

### many-generators

Observations: 329,168. Null producer: 14.07 ms.
Observations: 329,168. Null producer: 13.34 ms.
Observations: 329,168. Null producer: 13.34 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 52.61 → 49.71 → 50.14 | 65.17 → 64.07 → 63.67 |
| Write throughput (Mobs/s) | 6.257 → 6.622 → 6.565 | 5.051 → 5.138 → 5.170 |
| Read time (ms) | 37.08 → 35.08 → 34.16 | 20.32 → 19.98 → 19.98 |
| Read throughput (Mobs/s) | 8.877 → 9.384 → 9.635 | 16.201 → 16.473 → 16.477 |
| File size (MiB) | 1.662 → 1.662 → 1.662 | 7.571 → 7.571 → 7.571 |
| Bytes per observation | 5.29 → 5.29 → 5.29 | 24.12 → 24.12 → 24.12 |
| Logical bytes / file bytes | 12.84 → 12.84 → 12.84 | 2.82 → 2.82 → 2.82 |
| Write peak RSS (MiB) | 6.41 → 6.59 → 6.52 | 6.18 → 6.20 → 6.13 |
| Read peak RSS (MiB) | 6.92 → 7.10 → 7.08 | 11.95 → 11.99 → 12.03 |
| Capture time (ms) | 52.28 → 49.43 → 49.87 | 64.50 → 63.42 → 63.04 |
| Capture time (ns/tick) | 2178.4 → 2059.5 → 2077.9 | 2687.4 → 2642.6 → 2626.6 |
| File bytes per tick | 72.6 → 72.6 → 72.6 | 330.8 → 330.8 → 330.8 |
| Close time (ms) | 0.13 → 0.11 → 0.11 | 0.63 → 0.61 → 0.60 |
| Write CPU time (ms) | 53.34 → 50.29 → 50.70 | 65.99 → 64.62 → 64.23 |
| Write min / max (ms) | 50.75 / 54.58 → 49.64 / 49.82 → 49.48 / 50.66 | 64.79 / 67.04 → 63.74 / 64.67 → 63.43 / 64.59 |
| search-transaction: probes | 36 → 36 → 36 | 36 → 36 → 36 |
| search-transaction: matches | 15 → 15 → 15 | 15 → 15 → 15 |
| search-transaction: p50 (ms) | 0.290 → 0.282 → 0.279 | 0.599 → 0.580 → 0.575 |
| search-transaction: p95 (ms) | 0.390 → 0.386 → 0.374 | 1.360 → 1.276 → 1.286 |
| search-transaction: peak RSS (MiB) | 7.04 → 7.18 → 7.17 | 11.83 → 11.98 → 12.00 |
| find-transaction: probes | 36 → 36 → 36 | 36 → 36 → 36 |
| find-transaction: matches | 537 → 537 → 537 | 537 → 537 → 537 |
| find-transaction: p50 (ms) | 12.019 → 11.558 → 11.808 | 1.276 → 1.245 → 1.235 |
| find-transaction: p95 (ms) | 14.571 → 13.349 → 13.930 | 1.347 → 1.312 → 1.312 |
| find-transaction: peak RSS (MiB) | 7.02 → 7.17 → 7.09 | 11.93 → 11.96 → 11.96 |

### mixed

Observations: 1,341,285. Null producer: 11.97 ms.
Observations: 1,341,285. Null producer: 11.64 ms.
Observations: 1,341,285. Null producer: 11.50 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 36.88 → 34.83 → 34.91 | 84.24 → 81.75 → 81.04 | 48.53 → 46.24 → 46.12 |
| Write throughput (Mobs/s) | 36.367 → 38.509 → 38.416 | 15.921 → 16.408 → 16.552 | 27.639 → 29.010 → 29.085 |
| Read time (ms) | 16.27 → 15.65 → 15.42 | 37.87 → 37.89 → 36.68 | 40.72 → 40.19 → 39.59 |
| Read throughput (Mobs/s) | 82.428 → 85.718 → 86.966 | 35.414 → 35.401 → 36.569 | 32.942 → 33.373 → 33.879 |
| File size (MiB) | 1.970 → 1.970 → 1.970 | 4.010 → 4.010 → 4.010 | 31.812 → 31.812 → 31.812 |
| Bytes per observation | 1.54 → 1.54 → 1.54 | 3.13 → 3.13 → 3.13 | 24.87 → 24.87 → 24.87 |
| Logical bytes / file bytes | 11.98 → 11.98 → 11.98 | 5.89 → 5.89 → 5.89 | 0.74 → 0.74 → 0.74 |
| Write peak RSS (MiB) | 15.57 → 15.68 → 15.71 | 41.05 → 41.11 → 41.09 | 6.22 → 6.08 → 6.14 |
| Read peak RSS (MiB) | 7.42 → 7.48 → 7.45 | 9.38 → 9.39 → 9.46 | 6.12 → 6.19 → 6.13 |
| Capture time (ms) | 32.56 → 30.88 → 31.02 | 44.60 → 43.55 → 43.00 | 47.83 → 45.58 → 45.51 |
| Capture time (ns/tick) | 1356.5 → 1286.8 → 1292.6 | 1858.3 → 1814.8 → 1791.6 | 1992.9 → 1899.3 → 1896.2 |
| File bytes per tick | 86.1 → 86.1 → 86.1 | 175.2 → 175.2 → 175.2 | 1389.9 → 1389.9 → 1389.9 |
| Close time (ms) | 4.09 → 3.75 → 3.69 | 37.49 → 38.44 → 37.89 | 0.23 → 0.22 → 0.20 |
| Write CPU time (ms) | 37.50 → 35.36 → 35.47 | 84.86 → 82.29 → 81.55 | 49.25 → 46.77 → 46.63 |
| Write min / max (ms) | 35.56 / 37.62 → 34.71 / 35.13 → 34.63 / 34.94 | 82.22 / 84.68 → 81.68 / 82.40 → 80.86 / 81.77 | 48.50 / 49.60 → 46.11 / 46.47 → 45.87 / 46.48 |
| search-signal: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| search-signal: matches | 294 → 294 → 294 | 294 → 294 → 294 | 294 → 294 → 294 |
| search-signal: p50 (ms) | 0.081 → 0.094 → 0.080 | 0.217 → 0.212 → 0.193 | 11.926 → 11.254 → 11.276 |
| search-signal: p95 (ms) | 0.232 → 0.220 → 0.202 | 1.925 → 1.748 → 1.650 | 16.788 → 16.392 → 15.885 |
| search-signal: peak RSS (MiB) | 6.63 → 6.76 → 6.71 | 9.88 → 9.86 → 9.84 | 6.61 → 6.69 → 6.71 |
| find-signal: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| find-signal: matches | 53,496 → 53,496 → 53,496 | 53,496 → 53,496 → 53,496 | 53,496 → 53,496 → 53,496 |
| find-signal: p50 (ms) | 0.098 → 0.099 → 0.094 | 0.203 → 0.197 → 0.187 | 17.777 → 17.212 → 17.040 |
| find-signal: p95 (ms) | 0.246 → 0.235 → 0.233 | 1.656 → 1.580 → 1.566 | 18.205 → 17.926 → 17.829 |
| find-signal: peak RSS (MiB) | 6.60 → 6.73 → 6.69 | 9.82 → 9.82 → 9.81 | 6.61 → 6.68 → 6.68 |
| point: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| point: matches | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| point: p50 (ms) | 0.084 → 0.086 → 0.083 | 0.034 → 0.035 → 0.034 | 11.505 → 11.336 → 11.058 |
| point: p95 (ms) | 0.220 → 0.210 → 0.206 | 0.134 → 0.129 → 0.123 | 16.566 → 16.287 → 15.906 |
| point: peak RSS (MiB) | 6.61 → 6.71 → 6.78 | 5.18 → 5.23 → 5.20 | 6.67 → 6.66 → 6.65 |
| search-transaction: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| search-transaction: matches | 24 → 24 → 24 | 24 → 24 → 24 | 24 → 24 → 24 |
| search-transaction: p50 (ms) | 0.144 → 0.141 → 0.142 | 0.426 → 0.417 → 0.419 | 0.425 → 0.415 → 0.410 |
| search-transaction: p95 (ms) | 0.201 → 0.203 → 0.200 | 1.084 → 1.057 → 1.055 | 1.043 → 1.053 → 1.011 |
| search-transaction: peak RSS (MiB) | 5.73 → 5.90 → 5.77 | 6.00 → 6.18 → 6.18 | 6.11 → 6.17 → 6.15 |
| find-transaction: probes | 36 → 36 → 36 | 36 → 36 → 36 | 36 → 36 → 36 |
| find-transaction: matches | 543 → 543 → 543 | 543 → 543 → 543 | 543 → 543 → 543 |
| find-transaction: p50 (ms) | 3.686 → 3.483 → 3.676 | 1.070 → 1.035 → 1.044 | 1.067 → 1.033 → 1.028 |
| find-transaction: p95 (ms) | 4.688 → 4.559 → 4.547 | 1.140 → 1.069 → 1.080 | 1.140 → 1.081 → 1.087 |
| find-transaction: peak RSS (MiB) | 5.88 → 5.96 → 5.93 | 6.10 → 6.09 → 6.17 | 6.14 → 6.17 → 6.21 |

## Correctness and integration gates

- Native Release configure/build and all 38 CTests pass, including bidirectional Rust/C++ interoperability, codecs, inline/threaded execution, encoding workers, recovery, sidecars, fixed schemas and embedded records.
- Rust workspace tests, all-target checks, formatting, Clippy with all features and warnings denied, and documentation with warnings denied pass. Volna trace/server/core tests pass.
- Mixed-schema tests cover shared datatypes, one logger with different layouts, one callsite shared by loggers, duplicate occurrences, owner queries, retained views, borrowed-input mutation, disabled side effects, concurrent registration and sequential recordings. Raw capture rejects unknown/wrong callsites, truncated/trailing payloads and malformed declaration metadata. Declaration-order tests reject later repairs; selective queries avoid corrupt unselected pages.
- All 35 tracked VTR fixtures are format 3.3. The complete eight-case Verilator pipeline matrix passes. All six applicable VTR-specific Verilator regression scenarios pass.
- Address/undefined-behavior sanitizer checks pass for native interop and retained payloads. Leak detection is disabled because the execution sandbox prevents LeakSanitizer from tracing its threads.
- The installed C++ 5 package configures, builds and runs its independent consumer, including retained payload decoding after reader destruction.
- Documentation check/build/browser tests pass: 52 route/theme/viewport combinations, 26 accessibility audits, import/error recovery, million-row sorting/filtering, cancellation and static hosting. Light desktop and dark 360px screenshots were inspected.
- Verilator `make format`, `make lint-py` and `make cppcheck` complete successfully. Cppcheck emits diagnostics already present in unrelated compiler code; its configured check has no fatal analysis error.

The full Verilator longtest run is **not green**: 6,335 passed, 16 failed and 256 skipped. Targeted reruns pass the examples and default-parameter tests after clearing relocated generated caches, both single-CPU benchmark cases and the NUMA case with full CPU affinity, and Python lint with its tools installed. The remaining ten failures reproduce against unchanged Verilator `6eb99ee3ac238b0373707660aa7708122741933f`:

| Existing failing checks | Baseline result |
|---|---|
| cinclude, copyright, fixme, whitespace | Harness opens the `ext/slang` submodule directory as a text file |
| cppstyle | Existing `src/vdb_index/Json.h` enum lacks an explicit underlying type |
| docs_options, docs_style | Existing `--trace-vtr` references/order disagree with the distribution checker |
| warn_coverage | Existing “Cannot write VDB companion” diagnostic lacks a matching golden-output test |
| contributors | Existing commit author is absent from the human-maintained contributor list |
| verilated_all | Existing uncovered-header check calls `VlTest.error` with the wrong argument count |

These baseline checks were reproduced without editing their source or golden files. Contributor-list edits are expressly reserved for humans by Verilator’s guidelines. Their failures are recorded rather than reported as passing.
