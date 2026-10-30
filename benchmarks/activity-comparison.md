# Generator activity validation

The baseline is VTR `4c818bc46ee2935dcace42c6e6347a89f4fe96ad`; the candidate adds
VTI 4.0 generator intervals. Both complete default suites ran on the same Linux
image (`7208850ec8f1`), CPU 0, three repetitions, and full workload sizes.
Baseline elapsed time was 124.60 s; the final candidate suite took 56.80 s.
These elapsed times include other formats and do not establish a VTR speedup.

All 490 shared metric cells were compared with the previously committed report,
including capture/close costs, CPU time, RSS, file size, reads, and queries for
every format and workload. File sizes and all independently verified results
were unchanged. Timing and RSS changes also occurred in unchanged FST/FTR/VCD
backends. Two seven-repeat paired VTR comparisons and a seven-repeat identical-
binary control investigated this variation on the same image and CPU.

In the final paired comparison below, write-time changes ranged from -18.2% to
+0.1% and full-read changes from -27.4% to +0.6%. Query p50 changes ranged from
-8.3% to +24.3%; p95 changes ranged from -50.0% to +49.1%. Core signal-search
p50 was +24.3% in this comparison and -1.6% in the first comparison. Earlier
close-time outliers (+50.4% core, +263.6% many-generators) became -5.7% and
+0.7%, respectively. The identical-binary control produced read-time changes
from -2.6% to +13.2%, query p95 changes from -13.9% to +11.5%, and RSS changes
from -29.0% to +40.8%. A stable timing regression was not isolated; finer
performance bounds require a less variable host. The complete measurements
below preserve slower results as well as faster ones.

The default suite does not build VTI sidecars. Separate activity tests compare
exact generator windows against decoded transactions, events, stages, and logs,
including scalar/general pages, out-of-order times, gaps, zero, and u64::MAX.
Rust/C++ interoperability checks exercise both sidecar directions for every
codec and writer mode. Volna tests cover generator-only scopes, variable-list
rows in both themes, and remote classification without another index request
while panning.

The interval-union test indexes 100,000 overlapping intervals with a 4 KiB
scan-admission estimate and produces a sidecar smaller than 1 KiB. It also
checks all 150,001 point windows around 50,000 disjoint intervals; the decoded
index occupies less than 2 MiB. Memory denial and cancellation are exercised
while scanning and decoding generator intervals. These are admission estimates,
not process-wide allocator caps. Payload-allocation limits do not affect the
metadata-only scan. Construction retains disjoint intervals; classification
uses binary search per generator.

## Paired measurements

Status: **complete; all checks passed**. Measured 2026-10-30T19:29:29.022204+00:00. Elapsed: 26.00 s.

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Paired repetitions: 7. Full default workload sizes.

Each pair runs both binaries in shuffled order on one CPU. Each binary writes its own trace, then runs the full read and all twelve probes of every applicable query. Every result is validated against both binaries' independent producer replays. Timings include complete writes, full reads, and fresh-reader queries; lower time and RSS are better. Changes below are candidate / baseline − 1. Small differences and process RSS variation are not evidence of a speedup or regression.

Both executables use their own source replay and files. Preserve build provenance and compare every metric, including slower results. See the benchmark guide for the shared workload and timing boundaries.

Baseline: baseline. Binary SHA-256: `cbb9548bd45ac1ab35883adb23cda57d726b5cdffa0297db1e17bc348f75ae8e`.

Candidate: candidate. Binary SHA-256: `23f5bd48e2ed3f16f128d53f519eb8a5acf93a042bfbb79af0680d25a0c2781a`.

### core

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 136.29 | 130.83 | -4.0% |
| Write throughput (Mobs/s) | 46.158 | 48.087 | +4.2% |
| File size (MiB) | 7.695 | 7.695 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.86 | 11.86 | +0.0% |
| Write peak RSS (MiB) | 21.72 | 21.62 | -0.5% |
| Capture time (ms) | 133.55 | 127.64 | -4.4% |
| Capture time (ns/tick) | 278.2 | 265.9 | -4.4% |
| File bytes per tick | 16.8 | 16.8 | +0.0% |
| Close time (ms) | 3.34 | 3.15 | -5.7% |
| Write CPU time (ms) | 123.91 | 119.72 | -3.4% |
| Write min / max (ms) | 110.77 / 184.95 | 108.75 / 188.03 | — |
| Read time (ms) | 71.19 | 69.33 | -2.6% |
| Read throughput (Mobs/s) | 88.376 | 90.748 | +2.7% |
| Read peak RSS (MiB) | 13.93 | 14.01 | +0.6% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 679 | 679 | +0.0% |
| search-signal: p50 (ms) | 0.802 | 0.997 | +24.3% |
| search-signal: p95 (ms) | 1.995 | 2.743 | +37.5% |
| search-signal: peak RSS (MiB) | 7.35 | 7.09 | -3.5% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 2,493,463 | 2,493,463 | +0.0% |
| find-signal: p50 (ms) | 3.051 | 2.799 | -8.3% |
| find-signal: p95 (ms) | 7.484 | 10.279 | +37.3% |
| find-signal: peak RSS (MiB) | 7.95 | 8.39 | +5.5% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.979 | 0.934 | -4.6% |
| point: p95 (ms) | 2.195 | 2.936 | +33.8% |
| point: peak RSS (MiB) | 7.29 | 7.51 | +3.0% |

### soc

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 594.46 | 486.13 | -18.2% |
| Write throughput (Mobs/s) | 33.886 | 41.437 | +22.3% |
| File size (MiB) | 24.579 | 24.579 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.89 | 11.89 | +0.0% |
| Write peak RSS (MiB) | 17.68 | 17.66 | -0.1% |
| Capture time (ms) | 581.64 | 473.26 | -18.6% |
| Capture time (ns/tick) | 24235.1 | 19719.2 | -18.6% |
| File bytes per tick | 1073.9 | 1073.9 | +0.0% |
| Close time (ms) | 10.89 | 11.69 | +7.3% |
| Write CPU time (ms) | 549.86 | 464.30 | -15.6% |
| Write min / max (ms) | 450.33 / 666.51 | 446.34 / 664.68 | — |
| Read time (ms) | 221.58 | 160.79 | -27.4% |
| Read throughput (Mobs/s) | 90.909 | 125.280 | +37.8% |
| Read peak RSS (MiB) | 31.09 | 31.07 | -0.1% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 693 | 693 | +0.0% |
| search-signal: p50 (ms) | 1.163 | 1.181 | +1.5% |
| search-signal: p95 (ms) | 2.689 | 4.010 | +49.1% |
| search-signal: peak RSS (MiB) | 8.18 | 8.26 | +1.0% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 124,873 | 124,873 | +0.0% |
| find-signal: p50 (ms) | 2.533 | 2.517 | -0.6% |
| find-signal: p95 (ms) | 7.503 | 6.168 | -17.8% |
| find-signal: peak RSS (MiB) | 10.18 | 10.25 | +0.7% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 1.200 | 1.126 | -6.2% |
| point: p95 (ms) | 3.789 | 2.162 | -42.9% |
| point: peak RSS (MiB) | 8.06 | 8.17 | +1.4% |

### idle

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 66.17 | 63.01 | -4.8% |
| Write throughput (Mobs/s) | 38.496 | 40.426 | +5.0% |
| File size (MiB) | 1.732 | 1.732 | +0.0% |
| Bytes per observation | 0.71 | 0.71 | +0.0% |
| Logical bytes / file bytes | 21.34 | 21.34 | +0.0% |
| Write peak RSS (MiB) | 16.87 | 16.79 | -0.5% |
| Capture time (ms) | 52.52 | 53.59 | +2.0% |
| Capture time (ns/tick) | 1094.2 | 1116.4 | +2.0% |
| File bytes per tick | 37.8 | 37.8 | +0.0% |
| Close time (ms) | 10.92 | 9.13 | -16.4% |
| Write CPU time (ms) | 60.97 | 61.65 | +1.1% |
| Write min / max (ms) | 62.05 / 67.86 | 61.15 / 69.86 | — |
| Read time (ms) | 20.59 | 20.72 | +0.6% |
| Read throughput (Mobs/s) | 123.695 | 122.942 | -0.6% |
| Read peak RSS (MiB) | 8.54 | 8.48 | -0.7% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 7 | 7 | +0.0% |
| search-signal: p50 (ms) | 0.547 | 0.553 | +1.1% |
| search-signal: p95 (ms) | 1.001 | 0.884 | -11.7% |
| search-signal: peak RSS (MiB) | 7.41 | 7.41 | +0.0% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 15,736 | 15,736 | +0.0% |
| find-signal: p50 (ms) | 0.656 | 0.652 | -0.6% |
| find-signal: p95 (ms) | 1.035 | 0.953 | -7.9% |
| find-signal: peak RSS (MiB) | 7.66 | 7.69 | +0.4% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.549 | 0.535 | -2.6% |
| point: p95 (ms) | 0.797 | 0.746 | -6.4% |
| point: peak RSS (MiB) | 7.41 | 7.59 | +2.4% |

### requests

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 34.58 | 33.99 | -1.7% |
| Write throughput (Mobs/s) | 5.949 | 6.053 | +1.7% |
| File size (MiB) | 1.046 | 1.046 | +0.0% |
| Bytes per observation | 5.33 | 5.33 | +0.0% |
| Logical bytes / file bytes | 12.75 | 12.75 | +0.0% |
| Write peak RSS (MiB) | 7.35 | 5.85 | -20.4% |
| Capture time (ms) | 33.52 | 33.29 | -0.7% |
| Capture time (ns/tick) | 279.3 | 277.4 | -0.7% |
| File bytes per tick | 9.1 | 9.1 | +0.0% |
| Close time (ms) | 0.39 | 0.37 | -5.1% |
| Write CPU time (ms) | 30.04 | 29.64 | -1.3% |
| Write min / max (ms) | 33.34 / 37.88 | 32.64 / 36.43 | — |
| Read time (ms) | 17.02 | 16.96 | -0.4% |
| Read throughput (Mobs/s) | 12.085 | 12.128 | +0.4% |
| Read peak RSS (MiB) | 7.09 | 5.14 | -27.5% |
| search-transaction: probes | 84 | 84 | +0.0% |
| search-transaction: matches | 63 | 63 | +0.0% |
| search-transaction: p50 (ms) | 0.568 | 0.576 | +1.4% |
| search-transaction: p95 (ms) | 1.478 | 0.739 | -50.0% |
| search-transaction: peak RSS (MiB) | 5.01 | 7.05 | +40.7% |
| find-transaction: probes | 84 | 84 | +0.0% |
| find-transaction: matches | 6,335 | 6,335 | +0.0% |
| find-transaction: p50 (ms) | 5.096 | 5.433 | +6.6% |
| find-transaction: p95 (ms) | 5.578 | 5.896 | +5.7% |
| find-transaction: peak RSS (MiB) | 7.12 | 7.12 | +0.0% |

### many-generators

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 64.42 | 63.04 | -2.1% |
| Write throughput (Mobs/s) | 5.110 | 5.222 | +2.2% |
| File size (MiB) | 1.662 | 1.662 | +0.0% |
| Bytes per observation | 5.29 | 5.29 | +0.0% |
| Logical bytes / file bytes | 12.84 | 12.84 | +0.0% |
| Write peak RSS (MiB) | 6.03 | 6.80 | +12.8% |
| Capture time (ms) | 60.95 | 60.56 | -0.6% |
| Capture time (ns/tick) | 2539.4 | 2523.5 | -0.6% |
| File bytes per tick | 72.6 | 72.6 | +0.0% |
| Close time (ms) | 2.84 | 2.86 | +0.7% |
| Write CPU time (ms) | 54.85 | 54.43 | -0.8% |
| Write min / max (ms) | 58.74 / 66.03 | 61.81 / 64.31 | — |
| Read time (ms) | 28.37 | 28.44 | +0.2% |
| Read throughput (Mobs/s) | 11.601 | 11.574 | -0.2% |
| Read peak RSS (MiB) | 5.71 | 7.82 | +37.0% |
| search-transaction: probes | 84 | 84 | +0.0% |
| search-transaction: matches | 35 | 35 | +0.0% |
| search-transaction: p50 (ms) | 0.962 | 0.945 | -1.8% |
| search-transaction: p95 (ms) | 1.374 | 1.160 | -15.6% |
| search-transaction: peak RSS (MiB) | 5.71 | 5.84 | +2.3% |
| find-transaction: probes | 84 | 84 | +0.0% |
| find-transaction: matches | 1,253 | 1,253 | +0.0% |
| find-transaction: p50 (ms) | 6.611 | 6.769 | +2.4% |
| find-transaction: p95 (ms) | 7.366 | 7.714 | +4.7% |
| find-transaction: peak RSS (MiB) | 7.71 | 7.95 | +3.1% |

### mixed

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 38.77 | 38.82 | +0.1% |
| Write throughput (Mobs/s) | 34.594 | 34.550 | -0.1% |
| File size (MiB) | 1.970 | 1.970 | +0.0% |
| Bytes per observation | 1.54 | 1.54 | +0.0% |
| Logical bytes / file bytes | 11.98 | 11.98 | +0.0% |
| Write peak RSS (MiB) | 17.06 | 16.33 | -4.3% |
| Capture time (ms) | 31.95 | 32.10 | +0.5% |
| Capture time (ns/tick) | 1331.3 | 1337.5 | +0.5% |
| File bytes per tick | 86.1 | 86.1 | +0.0% |
| Close time (ms) | 6.24 | 5.99 | -4.0% |
| Write CPU time (ms) | 34.10 | 34.18 | +0.2% |
| Write min / max (ms) | 36.58 / 39.78 | 36.19 / 39.52 | — |
| Read time (ms) | 16.33 | 16.38 | +0.3% |
| Read throughput (Mobs/s) | 82.126 | 81.904 | -0.3% |
| Read peak RSS (MiB) | 8.33 | 8.31 | -0.2% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 686 | 686 | +0.0% |
| search-signal: p50 (ms) | 0.401 | 0.409 | +2.0% |
| search-signal: p95 (ms) | 0.756 | 0.762 | +0.8% |
| search-signal: peak RSS (MiB) | 6.88 | 5.05 | -26.6% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 124,824 | 124,824 | +0.0% |
| find-signal: p50 (ms) | 0.435 | 0.430 | -1.1% |
| find-signal: p95 (ms) | 0.747 | 0.623 | -16.6% |
| find-signal: peak RSS (MiB) | 7.03 | 6.96 | -1.0% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.398 | 0.417 | +4.8% |
| point: p95 (ms) | 0.760 | 0.525 | -30.9% |
| point: peak RSS (MiB) | 6.80 | 6.96 | +2.4% |
| search-transaction: probes | 84 | 84 | +0.0% |
| search-transaction: matches | 56 | 56 | +0.0% |
| search-transaction: p50 (ms) | 0.326 | 0.329 | +0.9% |
| search-transaction: p95 (ms) | 0.607 | 0.460 | -24.2% |
| search-transaction: peak RSS (MiB) | 6.62 | 4.76 | -28.1% |
| find-transaction: probes | 84 | 84 | +0.0% |
| find-transaction: matches | 1,267 | 1,267 | +0.0% |
| find-transaction: p50 (ms) | 1.921 | 2.021 | +5.2% |
| find-transaction: p95 (ms) | 2.254 | 2.228 | -1.2% |
| find-transaction: peak RSS (MiB) | 4.70 | 6.61 | +40.6% |
