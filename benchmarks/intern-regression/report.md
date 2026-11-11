# Native VTR paired comparison

Status: **complete; all checks passed**. Measured 2026-11-10T21:49:36.668837+00:00. Elapsed: 42.84 s.

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Paired repetitions: 15. Full default workload sizes.

Each pair runs both binaries in shuffled order on one CPU. Each binary writes its own trace, then runs the full read and all twelve probes of every applicable query. Every result is validated against both binaries' independent producer replays. Timings include complete writes, full reads, and fresh-reader queries; lower time and RSS are better. Changes below are candidate / baseline − 1. Small differences and process RSS variation are not evidence of a speedup or regression.

Both executables use their own source replay and files. Preserve build provenance and compare every metric, including slower results. See the benchmark guide for the shared workload and timing boundaries.

Baseline: 5d0e1c5. Binary SHA-256: `ef0eece6c709af2af6bf4a6012af4fc7474b8d25a6fc031d61ee3078986e5a0a`.

Candidate: interned-values. Binary SHA-256: `6044ea5deba62bd78ad5e49ab92a11bf3ebb94be1d78a4285148d787397e712a`.

## core

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 110.44 | 103.28 | -6.5% |
| Write throughput (Mobs/s) | 56.962 | 60.912 | +6.9% |
| File size (MiB) | 7.695 | 7.695 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.86 | 11.86 | +0.0% |
| Write peak RSS (MiB) | 21.72 | 21.57 | -0.7% |
| Capture time (ms) | 108.08 | 100.99 | -6.6% |
| Capture time (ns/tick) | 225.2 | 210.4 | -6.6% |
| File bytes per tick | 16.8 | 16.8 | +0.0% |
| Close time (ms) | 2.24 | 2.09 | -6.7% |
| Write CPU time (ms) | 105.50 | 100.23 | -5.0% |
| Write min / max (ms) | 103.16 / 152.75 | 100.66 / 123.11 | — |
| Read time (ms) | 48.29 | 49.68 | +2.9% |
| Read throughput (Mobs/s) | 130.265 | 126.642 | -2.8% |
| Read peak RSS (MiB) | 13.61 | 13.61 | +0.0% |
| search-signal: probes | 180 | 180 | +0.0% |
| search-signal: matches | 1,455 | 1,455 | +0.0% |
| search-signal: p50 (ms) | 0.601 | 0.588 | -2.2% |
| search-signal: p95 (ms) | 0.901 | 0.896 | -0.6% |
| search-signal: peak RSS (MiB) | 7.64 | 7.57 | -0.9% |
| find-signal: probes | 180 | 180 | +0.0% |
| find-signal: matches | 5,343,135 | 5,343,135 | +0.0% |
| find-signal: p50 (ms) | 1.828 | 1.858 | +1.6% |
| find-signal: p95 (ms) | 4.212 | 4.276 | +1.5% |
| find-signal: peak RSS (MiB) | 8.46 | 8.38 | -0.9% |
| point: probes | 180 | 180 | +0.0% |
| point: matches | 180 | 180 | +0.0% |
| point: p50 (ms) | 0.588 | 0.593 | +0.9% |
| point: p95 (ms) | 0.927 | 0.907 | -2.2% |
| point: peak RSS (MiB) | 7.48 | 7.42 | -0.8% |

## soc

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 416.85 | 411.41 | -1.3% |
| Write throughput (Mobs/s) | 48.324 | 48.963 | +1.3% |
| File size (MiB) | 24.579 | 24.579 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.89 | 11.89 | +0.0% |
| Write peak RSS (MiB) | 17.73 | 17.48 | -1.4% |
| Capture time (ms) | 406.13 | 401.95 | -1.0% |
| Capture time (ns/tick) | 16922.2 | 16748.0 | -1.0% |
| File bytes per tick | 1073.9 | 1073.9 | +0.0% |
| Close time (ms) | 9.50 | 8.80 | -7.4% |
| Write CPU time (ms) | 395.54 | 395.39 | -0.0% |
| Write min / max (ms) | 400.73 / 465.08 | 398.94 / 563.43 | — |
| Read time (ms) | 149.87 | 146.74 | -2.1% |
| Read throughput (Mobs/s) | 134.412 | 137.274 | +2.1% |
| Read peak RSS (MiB) | 31.16 | 30.91 | -0.8% |
| search-signal: probes | 180 | 180 | +0.0% |
| search-signal: matches | 1,485 | 1,485 | +0.0% |
| search-signal: p50 (ms) | 0.983 | 1.001 | +1.8% |
| search-signal: p95 (ms) | 2.319 | 2.342 | +1.0% |
| search-signal: peak RSS (MiB) | 8.27 | 8.28 | +0.1% |
| find-signal: probes | 180 | 180 | +0.0% |
| find-signal: matches | 267,585 | 267,585 | +0.0% |
| find-signal: p50 (ms) | 2.105 | 2.076 | -1.4% |
| find-signal: p95 (ms) | 2.662 | 2.635 | -1.0% |
| find-signal: peak RSS (MiB) | 10.45 | 10.50 | +0.5% |
| point: probes | 180 | 180 | +0.0% |
| point: matches | 180 | 180 | +0.0% |
| point: p50 (ms) | 0.979 | 0.985 | +0.6% |
| point: p95 (ms) | 1.505 | 1.409 | -6.4% |
| point: peak RSS (MiB) | 8.19 | 8.13 | -0.7% |

## idle

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 59.33 | 59.61 | +0.5% |
| Write throughput (Mobs/s) | 42.932 | 42.732 | -0.5% |
| File size (MiB) | 1.732 | 1.732 | +0.0% |
| Bytes per observation | 0.71 | 0.71 | +0.0% |
| Logical bytes / file bytes | 21.34 | 21.34 | +0.0% |
| Write peak RSS (MiB) | 16.80 | 16.73 | -0.4% |
| Capture time (ms) | 50.03 | 50.55 | +1.0% |
| Capture time (ns/tick) | 1042.3 | 1053.1 | +1.0% |
| File bytes per tick | 37.8 | 37.8 | +0.0% |
| Close time (ms) | 8.65 | 8.69 | +0.5% |
| Write CPU time (ms) | 55.03 | 56.13 | +2.0% |
| Write min / max (ms) | 54.75 / 63.95 | 55.75 / 67.17 | — |
| Read time (ms) | 19.60 | 19.28 | -1.6% |
| Read throughput (Mobs/s) | 129.934 | 132.130 | +1.7% |
| Read peak RSS (MiB) | 8.53 | 8.43 | -1.2% |
| search-signal: probes | 180 | 180 | +0.0% |
| search-signal: matches | 15 | 15 | +0.0% |
| search-signal: p50 (ms) | 0.515 | 0.529 | +2.7% |
| search-signal: p95 (ms) | 0.772 | 0.753 | -2.5% |
| search-signal: peak RSS (MiB) | 7.84 | 7.48 | -4.6% |
| find-signal: probes | 180 | 180 | +0.0% |
| find-signal: matches | 33,720 | 33,720 | +0.0% |
| find-signal: p50 (ms) | 0.628 | 0.631 | +0.5% |
| find-signal: p95 (ms) | 1.075 | 0.787 | -26.8% |
| find-signal: peak RSS (MiB) | 7.88 | 7.71 | -2.2% |
| point: probes | 180 | 180 | +0.0% |
| point: matches | 180 | 180 | +0.0% |
| point: p50 (ms) | 0.529 | 0.522 | -1.3% |
| point: p95 (ms) | 0.712 | 0.746 | +4.8% |
| point: peak RSS (MiB) | 7.47 | 7.70 | +3.1% |

## requests

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 35.88 | 35.91 | +0.1% |
| Write throughput (Mobs/s) | 5.734 | 5.729 | -0.1% |
| File size (MiB) | 1.046 | 1.046 | +0.0% |
| Bytes per observation | 5.33 | 5.33 | +0.0% |
| Logical bytes / file bytes | 12.75 | 12.75 | +0.0% |
| Write peak RSS (MiB) | 6.78 | 7.14 | +5.3% |
| Capture time (ms) | 34.00 | 33.86 | -0.4% |
| Capture time (ns/tick) | 283.4 | 282.1 | -0.5% |
| File bytes per tick | 9.1 | 9.1 | +0.0% |
| Close time (ms) | 1.06 | 1.42 | +34.0% |
| Write CPU time (ms) | 29.27 | 29.33 | +0.2% |
| Write min / max (ms) | 34.84 / 38.83 | 34.68 / 38.02 | — |
| Read time (ms) | 15.44 | 15.50 | +0.4% |
| Read throughput (Mobs/s) | 13.320 | 13.269 | -0.4% |
| Read peak RSS (MiB) | 7.19 | 7.11 | -1.1% |
| search-transaction: probes | 180 | 180 | +0.0% |
| search-transaction: matches | 135 | 135 | +0.0% |
| search-transaction: p50 (ms) | 0.596 | 0.593 | -0.5% |
| search-transaction: p95 (ms) | 0.672 | 0.656 | -2.4% |
| search-transaction: peak RSS (MiB) | 7.29 | 5.20 | -28.7% |
| find-transaction: probes | 180 | 180 | +0.0% |
| find-transaction: matches | 13,575 | 13,575 | +0.0% |
| find-transaction: p50 (ms) | 4.486 | 4.482 | -0.1% |
| find-transaction: p95 (ms) | 4.667 | 4.816 | +3.2% |
| find-transaction: peak RSS (MiB) | 7.38 | 7.17 | -2.8% |

## many-generators

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 62.33 | 63.19 | +1.4% |
| Write throughput (Mobs/s) | 5.281 | 5.210 | -1.3% |
| File size (MiB) | 1.662 | 1.662 | +0.0% |
| Bytes per observation | 5.29 | 5.29 | +0.0% |
| Logical bytes / file bytes | 12.84 | 12.84 | +0.0% |
| Write peak RSS (MiB) | 7.24 | 7.26 | +0.3% |
| Capture time (ms) | 59.82 | 60.12 | +0.5% |
| Capture time (ns/tick) | 2492.7 | 2504.9 | +0.5% |
| File bytes per tick | 72.6 | 72.6 | +0.0% |
| Close time (ms) | 1.79 | 1.42 | -20.7% |
| Write CPU time (ms) | 51.48 | 51.95 | +0.9% |
| Write min / max (ms) | 60.38 / 72.48 | 60.33 / 99.12 | — |
| Read time (ms) | 27.15 | 27.10 | -0.2% |
| Read throughput (Mobs/s) | 12.126 | 12.146 | +0.2% |
| Read peak RSS (MiB) | 7.75 | 7.83 | +1.0% |
| search-transaction: probes | 180 | 180 | +0.0% |
| search-transaction: matches | 75 | 75 | +0.0% |
| search-transaction: p50 (ms) | 0.979 | 0.974 | -0.5% |
| search-transaction: p95 (ms) | 1.094 | 1.103 | +0.8% |
| search-transaction: peak RSS (MiB) | 7.92 | 7.95 | +0.4% |
| find-transaction: probes | 180 | 180 | +0.0% |
| find-transaction: matches | 2,685 | 2,685 | +0.0% |
| find-transaction: p50 (ms) | 5.460 | 5.370 | -1.6% |
| find-transaction: p95 (ms) | 5.782 | 8.310 | +43.7% |
| find-transaction: peak RSS (MiB) | 7.88 | 7.84 | -0.5% |

## mixed

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 36.45 | 35.99 | -1.3% |
| Write throughput (Mobs/s) | 36.796 | 37.267 | +1.3% |
| File size (MiB) | 1.970 | 1.970 | +0.0% |
| Bytes per observation | 1.54 | 1.54 | +0.0% |
| Logical bytes / file bytes | 11.98 | 11.98 | +0.0% |
| Write peak RSS (MiB) | 17.02 | 17.23 | +1.2% |
| Capture time (ms) | 31.98 | 31.68 | -0.9% |
| Capture time (ns/tick) | 1332.5 | 1319.9 | -0.9% |
| File bytes per tick | 86.1 | 86.1 | +0.0% |
| Close time (ms) | 4.04 | 3.99 | -1.2% |
| Write CPU time (ms) | 33.09 | 32.88 | -0.6% |
| Write min / max (ms) | 35.69 / 41.77 | 35.64 / 39.84 | — |
| Read time (ms) | 16.01 | 16.10 | +0.6% |
| Read throughput (Mobs/s) | 83.777 | 83.329 | -0.5% |
| Read peak RSS (MiB) | 8.50 | 8.38 | -1.4% |
| search-signal: probes | 180 | 180 | +0.0% |
| search-signal: matches | 1,470 | 1,470 | +0.0% |
| search-signal: p50 (ms) | 0.418 | 0.420 | +0.5% |
| search-signal: p95 (ms) | 0.548 | 0.536 | -2.2% |
| search-signal: peak RSS (MiB) | 7.24 | 7.19 | -0.7% |
| find-signal: probes | 180 | 180 | +0.0% |
| find-signal: matches | 267,480 | 267,480 | +0.0% |
| find-signal: p50 (ms) | 0.434 | 0.436 | +0.5% |
| find-signal: p95 (ms) | 0.562 | 0.556 | -1.1% |
| find-signal: peak RSS (MiB) | 7.10 | 7.12 | +0.3% |
| point: probes | 180 | 180 | +0.0% |
| point: matches | 180 | 180 | +0.0% |
| point: p50 (ms) | 0.420 | 0.420 | +0.0% |
| point: p95 (ms) | 0.509 | 0.522 | +2.6% |
| point: peak RSS (MiB) | 6.82 | 7.08 | +3.8% |
| search-transaction: probes | 180 | 180 | +0.0% |
| search-transaction: matches | 120 | 120 | +0.0% |
| search-transaction: p50 (ms) | 0.337 | 0.340 | +0.9% |
| search-transaction: p95 (ms) | 0.405 | 0.402 | -0.7% |
| search-transaction: peak RSS (MiB) | 6.88 | 6.72 | -2.3% |
| find-transaction: probes | 180 | 180 | +0.0% |
| find-transaction: matches | 2,715 | 2,715 | +0.0% |
| find-transaction: p50 (ms) | 1.704 | 1.684 | -1.2% |
| find-transaction: p95 (ms) | 1.866 | 1.915 | +2.6% |
| find-transaction: peak RSS (MiB) | 4.89 | 6.81 | +39.3% |
