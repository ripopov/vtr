# Native VTR container tuning

Status: **complete; all checks passed**. Measured 2026-10-18T21:45:04.800410+00:00. Elapsed: 25.12 s.

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Paired repetitions: 7. Full default workload sizes.

Each pair runs both binaries in shuffled order on one CPU. Each binary writes its own trace, then runs the full read and all twelve probes of every applicable query. Every result is validated against both binaries' independent producer replays. Timings include the same work as [the format comparison](report.md); lower time and RSS are better. Changes below are candidate / baseline − 1. Small differences and process RSS variation are not evidence of a speedup or regression.

[Container selection, sources, license, and lifetime constraints](../vtr-cpp/third_party/README.md). Sparse tables use vendored unordered_dense maps/sets; dense signal IDs and byte dictionaries use indexed arrays. Transaction capture already uses indexed storage, so its steady-state performance is expected to change little.

Baseline: 3f7a764. Binary SHA-256: `c35e65293eaf3f2334f8c2a57343741df4304f9a7f68abafb567310f89411f82`.

Candidate: unordered_dense 5.3.2 and indexed aliases. Binary SHA-256: `9512050cc6af8e670b0c8dd5211d72119aa29f66dba6854a7f7d2679e97f7f91`.

## core

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 114.54 | 112.30 | -2.0% |
| Write throughput (Mobs/s) | 54.925 | 56.018 | +2.0% |
| File size (MiB) | 7.666 | 7.666 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.91 | 11.91 | +0.0% |
| Write peak RSS (MiB) | 53.04 | 51.68 | -2.6% |
| Capture time (ms) | 86.46 | 87.42 | +1.1% |
| Capture time (ns/tick) | 180.1 | 182.1 | +1.1% |
| File bytes per tick | 16.7 | 16.7 | +0.0% |
| Close time (ms) | 27.75 | 24.78 | -10.7% |
| Write CPU time (ms) | 115.01 | 113.31 | -1.5% |
| Write min / max (ms) | 112.88 / 120.99 | 111.10 / 114.21 | — |
| Read time (ms) | 43.72 | 42.12 | -3.7% |
| Read throughput (Mobs/s) | 143.904 | 149.351 | +3.8% |
| Read peak RSS (MiB) | 17.66 | 17.77 | +0.6% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 679 | 679 | +0.0% |
| search-signal: p50 (ms) | 0.474 | 0.450 | -5.1% |
| search-signal: p95 (ms) | 3.828 | 1.973 | -48.5% |
| search-signal: peak RSS (MiB) | 12.59 | 12.70 | +0.9% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 2,493,463 | 2,493,463 | +0.0% |
| find-signal: p50 (ms) | 0.763 | 0.703 | -7.9% |
| find-signal: p95 (ms) | 6.139 | 6.123 | -0.3% |
| find-signal: peak RSS (MiB) | 12.59 | 12.70 | +0.9% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.555 | 0.529 | -4.7% |
| point: p95 (ms) | 2.384 | 2.306 | -3.3% |
| point: peak RSS (MiB) | 12.59 | 12.71 | +1.0% |

## soc

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 387.55 | 364.07 | -6.1% |
| Write throughput (Mobs/s) | 51.977 | 55.330 | +6.5% |
| File size (MiB) | 24.806 | 24.806 | +0.0% |
| Bytes per observation | 1.29 | 1.29 | +0.0% |
| Logical bytes / file bytes | 11.78 | 11.78 | +0.0% |
| Write peak RSS (MiB) | 114.33 | 114.35 | +0.0% |
| Capture time (ms) | 360.33 | 347.40 | -3.6% |
| Capture time (ns/tick) | 15013.7 | 14475.0 | -3.6% |
| File bytes per tick | 1083.8 | 1083.8 | +0.0% |
| Close time (ms) | 25.97 | 15.46 | -40.5% |
| Write CPU time (ms) | 388.16 | 365.00 | -6.0% |
| Write min / max (ms) | 386.05 / 391.51 | 362.82 / 366.22 | — |
| Read time (ms) | 111.89 | 108.25 | -3.3% |
| Read throughput (Mobs/s) | 180.036 | 186.083 | +3.4% |
| Read peak RSS (MiB) | 30.06 | 30.18 | +0.4% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 693 | 693 | +0.0% |
| search-signal: p50 (ms) | 0.289 | 0.281 | -2.8% |
| search-signal: p95 (ms) | 0.485 | 0.469 | -3.3% |
| search-signal: peak RSS (MiB) | 6.38 | 6.45 | +1.1% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 124,873 | 124,873 | +0.0% |
| find-signal: p50 (ms) | 0.325 | 0.318 | -2.2% |
| find-signal: p95 (ms) | 0.585 | 0.580 | -0.9% |
| find-signal: peak RSS (MiB) | 6.52 | 6.66 | +2.1% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.304 | 0.291 | -4.3% |
| point: p95 (ms) | 0.488 | 0.489 | +0.2% |
| point: peak RSS (MiB) | 6.34 | 6.43 | +1.4% |

## idle

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 59.63 | 50.46 | -15.4% |
| Write throughput (Mobs/s) | 42.717 | 50.482 | +18.2% |
| File size (MiB) | 1.853 | 1.853 | +0.0% |
| Bytes per observation | 0.76 | 0.76 | +0.0% |
| Logical bytes / file bytes | 19.95 | 19.95 | +0.0% |
| Write peak RSS (MiB) | 22.05 | 22.21 | +0.7% |
| Capture time (ms) | 36.30 | 36.64 | +0.9% |
| Capture time (ns/tick) | 756.2 | 763.3 | +0.9% |
| File bytes per tick | 40.5 | 40.5 | +0.0% |
| Close time (ms) | 22.12 | 12.69 | -42.6% |
| Write CPU time (ms) | 60.62 | 51.36 | -15.3% |
| Write min / max (ms) | 59.23 / 66.76 | 50.13 / 61.26 | — |
| Read time (ms) | 16.42 | 15.35 | -6.5% |
| Read throughput (Mobs/s) | 155.145 | 165.971 | +7.0% |
| Read peak RSS (MiB) | 7.16 | 7.38 | +3.1% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 7 | 7 | +0.0% |
| search-signal: p50 (ms) | 0.338 | 0.332 | -1.8% |
| search-signal: p95 (ms) | 0.557 | 0.544 | -2.3% |
| search-signal: peak RSS (MiB) | 5.80 | 5.92 | +2.1% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 15,736 | 15,736 | +0.0% |
| find-signal: p50 (ms) | 0.345 | 0.338 | -2.0% |
| find-signal: p95 (ms) | 0.546 | 0.522 | -4.4% |
| find-signal: peak RSS (MiB) | 5.78 | 5.90 | +2.1% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.339 | 0.335 | -1.2% |
| point: p95 (ms) | 0.541 | 0.512 | -5.4% |
| point: peak RSS (MiB) | 5.80 | 5.91 | +1.9% |

## requests

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 30.89 | 30.32 | -1.8% |
| Write throughput (Mobs/s) | 6.661 | 6.786 | +1.9% |
| File size (MiB) | 2.292 | 2.292 | +0.0% |
| Bytes per observation | 11.68 | 11.68 | +0.0% |
| Logical bytes / file bytes | 5.82 | 5.82 | +0.0% |
| Write peak RSS (MiB) | 9.50 | 9.73 | +2.4% |
| Capture time (ms) | 29.64 | 29.17 | -1.6% |
| Capture time (ns/tick) | 247.0 | 243.1 | -1.6% |
| File bytes per tick | 20.0 | 20.0 | +0.0% |
| Close time (ms) | 1.09 | 1.03 | -5.5% |
| Write CPU time (ms) | 31.34 | 31.30 | -0.1% |
| Write min / max (ms) | 29.28 / 36.33 | 29.94 / 31.12 | — |
| Read time (ms) | 16.64 | 16.71 | +0.4% |
| Read throughput (Mobs/s) | 12.366 | 12.313 | -0.4% |
| Read peak RSS (MiB) | 7.63 | 7.71 | +1.0% |
| search-transaction: probes | 84 | 84 | +0.0% |
| search-transaction: matches | 63 | 63 | +0.0% |
| search-transaction: p50 (ms) | 1.944 | 1.925 | -1.0% |
| search-transaction: p95 (ms) | 2.460 | 2.416 | -1.8% |
| search-transaction: peak RSS (MiB) | 6.08 | 6.18 | +1.6% |
| find-transaction: probes | 84 | 84 | +0.0% |
| find-transaction: matches | 6,335 | 6,335 | +0.0% |
| find-transaction: p50 (ms) | 11.095 | 10.763 | -3.0% |
| find-transaction: p95 (ms) | 12.343 | 11.919 | -3.4% |
| find-transaction: peak RSS (MiB) | 7.64 | 7.72 | +1.0% |

## many-generators

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 55.48 | 55.97 | +0.9% |
| Write throughput (Mobs/s) | 5.933 | 5.881 | -0.9% |
| File size (MiB) | 3.775 | 3.775 | +0.0% |
| Bytes per observation | 12.02 | 12.02 | +0.0% |
| Logical bytes / file bytes | 5.66 | 5.66 | +0.0% |
| Write peak RSS (MiB) | 9.19 | 9.34 | +1.6% |
| Capture time (ms) | 54.09 | 54.59 | +0.9% |
| Capture time (ns/tick) | 2253.7 | 2274.8 | +0.9% |
| File bytes per tick | 164.9 | 164.9 | +0.0% |
| Close time (ms) | 1.30 | 1.29 | -0.8% |
| Write CPU time (ms) | 56.33 | 56.93 | +1.1% |
| Write min / max (ms) | 54.72 / 56.00 | 55.58 / 56.69 | — |
| Read time (ms) | 29.40 | 29.03 | -1.3% |
| Read throughput (Mobs/s) | 11.195 | 11.338 | +1.3% |
| Read peak RSS (MiB) | 9.14 | 9.28 | +1.5% |
| search-transaction: probes | 84 | 84 | +0.0% |
| search-transaction: matches | 35 | 35 | +0.0% |
| search-transaction: p50 (ms) | 1.920 | 1.949 | +1.5% |
| search-transaction: p95 (ms) | 3.768 | 3.803 | +0.9% |
| search-transaction: peak RSS (MiB) | 6.71 | 6.84 | +1.9% |
| find-transaction: probes | 84 | 84 | +0.0% |
| find-transaction: matches | 1,253 | 1,253 | +0.0% |
| find-transaction: p50 (ms) | 16.845 | 16.878 | +0.2% |
| find-transaction: p95 (ms) | 17.990 | 18.028 | +0.2% |
| find-transaction: peak RSS (MiB) | 9.14 | 9.25 | +1.2% |

## mixed

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 36.84 | 36.20 | -1.7% |
| Write throughput (Mobs/s) | 36.408 | 37.052 | +1.8% |
| File size (MiB) | 2.500 | 2.500 | +0.0% |
| Bytes per observation | 1.95 | 1.95 | +0.0% |
| Logical bytes / file bytes | 9.44 | 9.44 | +0.0% |
| Write peak RSS (MiB) | 17.14 | 17.23 | +0.5% |
| Capture time (ms) | 30.84 | 31.29 | +1.5% |
| Capture time (ns/tick) | 1285.0 | 1303.6 | +1.4% |
| File bytes per tick | 109.2 | 109.2 | +0.0% |
| Close time (ms) | 5.81 | 4.62 | -20.5% |
| Write CPU time (ms) | 37.78 | 37.15 | -1.7% |
| Write min / max (ms) | 36.75 / 37.47 | 35.72 / 36.73 | — |
| Read time (ms) | 15.29 | 14.87 | -2.7% |
| Read throughput (Mobs/s) | 87.721 | 90.221 | +2.8% |
| Read peak RSS (MiB) | 8.11 | 8.22 | +1.4% |
| search-signal: probes | 84 | 84 | +0.0% |
| search-signal: matches | 686 | 686 | +0.0% |
| search-signal: p50 (ms) | 0.057 | 0.055 | -3.5% |
| search-signal: p95 (ms) | 0.203 | 0.202 | -0.5% |
| search-signal: peak RSS (MiB) | 5.95 | 6.05 | +1.7% |
| find-signal: probes | 84 | 84 | +0.0% |
| find-signal: matches | 124,824 | 124,824 | +0.0% |
| find-signal: p50 (ms) | 0.069 | 0.066 | -4.3% |
| find-signal: p95 (ms) | 0.338 | 0.338 | +0.0% |
| find-signal: peak RSS (MiB) | 5.93 | 6.08 | +2.5% |
| point: probes | 84 | 84 | +0.0% |
| point: matches | 84 | 84 | +0.0% |
| point: p50 (ms) | 0.062 | 0.058 | -6.5% |
| point: p95 (ms) | 0.203 | 0.204 | +0.5% |
| point: peak RSS (MiB) | 5.95 | 6.05 | +1.7% |
| search-transaction: probes | 84 | 84 | +0.0% |
| search-transaction: matches | 56 | 56 | +0.0% |
| search-transaction: p50 (ms) | 2.061 | 2.063 | +0.1% |
| search-transaction: p95 (ms) | 2.687 | 2.718 | +1.2% |
| search-transaction: peak RSS (MiB) | 6.48 | 6.59 | +1.7% |
| find-transaction: probes | 84 | 84 | +0.0% |
| find-transaction: matches | 1,267 | 1,267 | +0.0% |
| find-transaction: p50 (ms) | 4.535 | 4.562 | +0.6% |
| find-transaction: p95 (ms) | 5.663 | 5.743 | +1.4% |
| find-transaction: peak RSS (MiB) | 6.46 | 6.52 | +0.9% |
