# Native VTR paired comparison

Status: **complete; all checks passed**. Measured 2026-11-10T21:50:19.543127+00:00. Elapsed: 42.67 s.

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Paired repetitions: 15. Full default workload sizes.

Each pair runs both binaries in shuffled order on one CPU. Each binary writes its own trace, then runs the full read and all twelve probes of every applicable query. Every result is validated against both binaries' independent producer replays. Timings include complete writes, full reads, and fresh-reader queries; lower time and RSS are better. Changes below are candidate / baseline − 1. Small differences and process RSS variation are not evidence of a speedup or regression.

Both executables use their own source replay and files. Preserve build provenance and compare every metric, including slower results. See the benchmark guide for the shared workload and timing boundaries.

Baseline: 5d0e1c5. Binary SHA-256: `ef0eece6c709af2af6bf4a6012af4fc7474b8d25a6fc031d61ee3078986e5a0a`.

Candidate: interned-values. Binary SHA-256: `6044ea5deba62bd78ad5e49ab92a11bf3ebb94be1d78a4285148d787397e712a`.

## core

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 105.56 | 106.81 | +1.2% |
| Write throughput (Mobs/s) | 59.597 | 58.899 | -1.2% |
| File size (MiB) | 7.695 | 7.695 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.86 | 11.86 | +0.0% |
| Write peak RSS (MiB) | 18.29 | 17.89 | -2.2% |
| Capture time (ms) | 102.93 | 104.00 | +1.0% |
| Capture time (ns/tick) | 214.4 | 216.7 | +1.1% |
| File bytes per tick | 16.8 | 16.8 | +0.0% |
| Close time (ms) | 2.41 | 2.47 | +2.5% |
| Write CPU time (ms) | 102.48 | 103.73 | +1.2% |
| Write min / max (ms) | 103.69 / 110.17 | 101.70 / 118.84 | — |
| Read time (ms) | 46.95 | 46.93 | -0.0% |
| Read throughput (Mobs/s) | 133.994 | 134.057 | +0.0% |
| Read peak RSS (MiB) | 12.27 | 12.33 | +0.5% |
| search-signal: probes | 180 | 180 | +0.0% |
| search-signal: matches | 1,455 | 1,455 | +0.0% |
| search-signal: p50 (ms) | 0.600 | 0.573 | -4.5% |
| search-signal: p95 (ms) | 0.899 | 0.874 | -2.8% |
| search-signal: peak RSS (MiB) | 6.00 | 5.93 | -1.2% |
| find-signal: probes | 180 | 180 | +0.0% |
| find-signal: matches | 5,343,135 | 5,343,135 | +0.0% |
| find-signal: p50 (ms) | 2.028 | 1.757 | -13.4% |
| find-signal: p95 (ms) | 5.805 | 4.084 | -29.6% |
| find-signal: peak RSS (MiB) | 6.87 | 6.80 | -1.0% |
| point: probes | 180 | 180 | +0.0% |
| point: matches | 180 | 180 | +0.0% |
| point: p50 (ms) | 0.597 | 0.625 | +4.7% |
| point: p95 (ms) | 0.905 | 0.988 | +9.2% |
| point: peak RSS (MiB) | 6.00 | 5.95 | -0.8% |

## soc

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 385.28 | 391.62 | +1.6% |
| Write throughput (Mobs/s) | 52.283 | 51.438 | -1.6% |
| File size (MiB) | 24.579 | 24.579 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.89 | 11.89 | +0.0% |
| Write peak RSS (MiB) | 15.66 | 15.62 | -0.3% |
| Capture time (ms) | 375.85 | 380.43 | +1.2% |
| Capture time (ns/tick) | 15660.5 | 15851.3 | +1.2% |
| File bytes per tick | 1073.9 | 1073.9 | +0.0% |
| Close time (ms) | 8.10 | 9.89 | +22.1% |
| Write CPU time (ms) | 364.47 | 363.78 | -0.2% |
| Write min / max (ms) | 376.65 / 543.15 | 375.96 / 452.95 | — |
| Read time (ms) | 147.41 | 146.75 | -0.4% |
| Read throughput (Mobs/s) | 136.648 | 137.266 | +0.5% |
| Read peak RSS (MiB) | 29.39 | 29.71 | +1.1% |
| search-signal: probes | 180 | 180 | +0.0% |
| search-signal: matches | 1,485 | 1,485 | +0.0% |
| search-signal: p50 (ms) | 1.008 | 1.037 | +2.9% |
| search-signal: p95 (ms) | 2.589 | 2.611 | +0.8% |
| search-signal: peak RSS (MiB) | 6.44 | 6.45 | +0.2% |
| find-signal: probes | 180 | 180 | +0.0% |
| find-signal: matches | 267,585 | 267,585 | +0.0% |
| find-signal: p50 (ms) | 2.155 | 2.153 | -0.1% |
| find-signal: p95 (ms) | 2.847 | 2.811 | -1.3% |
| find-signal: peak RSS (MiB) | 8.60 | 8.54 | -0.7% |
| point: probes | 180 | 180 | +0.0% |
| point: matches | 180 | 180 | +0.0% |
| point: p50 (ms) | 1.003 | 1.011 | +0.8% |
| point: p95 (ms) | 1.425 | 1.452 | +1.9% |
| point: peak RSS (MiB) | 6.44 | 6.46 | +0.3% |

## idle

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 53.83 | 54.24 | +0.8% |
| Write throughput (Mobs/s) | 47.317 | 46.958 | -0.8% |
| File size (MiB) | 1.732 | 1.732 | +0.0% |
| Bytes per observation | 0.71 | 0.71 | +0.0% |
| Logical bytes / file bytes | 21.34 | 21.34 | +0.0% |
| Write peak RSS (MiB) | 14.61 | 14.55 | -0.4% |
| Capture time (ms) | 45.07 | 45.27 | +0.4% |
| Capture time (ns/tick) | 938.9 | 943.0 | +0.4% |
| File bytes per tick | 37.8 | 37.8 | +0.0% |
| Close time (ms) | 7.98 | 8.10 | +1.5% |
| Write CPU time (ms) | 51.79 | 52.02 | +0.4% |
| Write min / max (ms) | 53.22 / 68.58 | 53.42 / 56.39 | — |
| Read time (ms) | 19.54 | 19.40 | -0.7% |
| Read throughput (Mobs/s) | 130.323 | 131.301 | +0.8% |
| Read peak RSS (MiB) | 7.00 | 6.67 | -4.7% |
| search-signal: probes | 180 | 180 | +0.0% |
| search-signal: matches | 15 | 15 | +0.0% |
| search-signal: p50 (ms) | 0.548 | 0.523 | -4.6% |
| search-signal: p95 (ms) | 0.795 | 0.783 | -1.5% |
| search-signal: peak RSS (MiB) | 5.83 | 5.80 | -0.5% |
| find-signal: probes | 180 | 180 | +0.0% |
| find-signal: matches | 33,720 | 33,720 | +0.0% |
| find-signal: p50 (ms) | 0.633 | 0.635 | +0.3% |
| find-signal: p95 (ms) | 0.813 | 0.817 | +0.5% |
| find-signal: peak RSS (MiB) | 5.98 | 5.86 | -2.0% |
| point: probes | 180 | 180 | +0.0% |
| point: matches | 180 | 180 | +0.0% |
| point: p50 (ms) | 0.562 | 0.552 | -1.8% |
| point: p95 (ms) | 0.756 | 0.746 | -1.3% |
| point: peak RSS (MiB) | 5.84 | 5.74 | -1.7% |

## requests

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 35.72 | 35.68 | -0.1% |
| Write throughput (Mobs/s) | 5.759 | 5.766 | +0.1% |
| File size (MiB) | 1.046 | 1.046 | +0.0% |
| Bytes per observation | 5.33 | 5.33 | +0.0% |
| Logical bytes / file bytes | 12.75 | 12.75 | +0.0% |
| Write peak RSS (MiB) | 5.92 | 5.86 | -1.0% |
| Capture time (ms) | 33.94 | 33.98 | +0.1% |
| Capture time (ns/tick) | 282.9 | 283.2 | +0.1% |
| File bytes per tick | 9.1 | 9.1 | +0.0% |
| Close time (ms) | 1.31 | 1.19 | -9.2% |
| Write CPU time (ms) | 29.32 | 29.39 | +0.2% |
| Write min / max (ms) | 34.90 / 47.05 | 34.96 / 39.26 | — |
| Read time (ms) | 15.52 | 15.70 | +1.2% |
| Read throughput (Mobs/s) | 13.260 | 13.107 | -1.2% |
| Read peak RSS (MiB) | 5.27 | 5.27 | +0.0% |
| search-transaction: probes | 180 | 180 | +0.0% |
| search-transaction: matches | 135 | 135 | +0.0% |
| search-transaction: p50 (ms) | 0.600 | 0.601 | +0.2% |
| search-transaction: p95 (ms) | 0.680 | 0.669 | -1.6% |
| search-transaction: peak RSS (MiB) | 5.27 | 5.24 | -0.6% |
| find-transaction: probes | 180 | 180 | +0.0% |
| find-transaction: matches | 13,575 | 13,575 | +0.0% |
| find-transaction: p50 (ms) | 4.454 | 4.471 | +0.4% |
| find-transaction: p95 (ms) | 5.163 | 4.844 | -6.2% |
| find-transaction: peak RSS (MiB) | 5.27 | 5.34 | +1.3% |

## many-generators

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 64.37 | 62.61 | -2.7% |
| Write throughput (Mobs/s) | 5.114 | 5.257 | +2.8% |
| File size (MiB) | 1.662 | 1.662 | +0.0% |
| Bytes per observation | 5.29 | 5.29 | +0.0% |
| Logical bytes / file bytes | 12.84 | 12.84 | +0.0% |
| Write peak RSS (MiB) | 6.00 | 5.94 | -1.0% |
| Capture time (ms) | 61.75 | 60.27 | -2.4% |
| Capture time (ns/tick) | 2573.0 | 2511.1 | -2.4% |
| File bytes per tick | 72.6 | 72.6 | +0.0% |
| Close time (ms) | 2.02 | 1.96 | -3.0% |
| Write CPU time (ms) | 51.62 | 52.00 | +0.7% |
| Write min / max (ms) | 60.92 / 101.12 | 60.66 / 72.48 | — |
| Read time (ms) | 27.15 | 27.25 | +0.4% |
| Read throughput (Mobs/s) | 12.126 | 12.082 | -0.4% |
| Read peak RSS (MiB) | 5.97 | 5.91 | -1.0% |
| search-transaction: probes | 180 | 180 | +0.0% |
| search-transaction: matches | 75 | 75 | +0.0% |
| search-transaction: p50 (ms) | 0.988 | 0.986 | -0.2% |
| search-transaction: p95 (ms) | 1.237 | 1.237 | +0.0% |
| search-transaction: peak RSS (MiB) | 5.97 | 5.91 | -1.0% |
| find-transaction: probes | 180 | 180 | +0.0% |
| find-transaction: matches | 2,685 | 2,685 | +0.0% |
| find-transaction: p50 (ms) | 5.536 | 5.395 | -2.5% |
| find-transaction: p95 (ms) | 6.934 | 7.160 | +3.3% |
| find-transaction: peak RSS (MiB) | 6.03 | 5.97 | -1.0% |

## mixed

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 38.48 | 38.42 | -0.2% |
| Write throughput (Mobs/s) | 34.856 | 34.913 | +0.2% |
| File size (MiB) | 1.970 | 1.970 | +0.0% |
| Bytes per observation | 1.54 | 1.54 | +0.0% |
| Logical bytes / file bytes | 11.98 | 11.98 | +0.0% |
| Write peak RSS (MiB) | 15.84 | 15.84 | +0.0% |
| Capture time (ms) | 33.36 | 33.52 | +0.5% |
| Capture time (ns/tick) | 1390.1 | 1396.8 | +0.5% |
| File bytes per tick | 86.1 | 86.1 | +0.0% |
| Close time (ms) | 4.57 | 4.70 | +2.8% |
| Write CPU time (ms) | 35.26 | 35.24 | -0.1% |
| Write min / max (ms) | 36.94 / 56.84 | 37.12 / 45.43 | — |
| Read time (ms) | 16.50 | 16.50 | +0.0% |
| Read throughput (Mobs/s) | 81.301 | 81.298 | -0.0% |
| Read peak RSS (MiB) | 6.57 | 6.44 | -2.0% |
| search-signal: probes | 180 | 180 | +0.0% |
| search-signal: matches | 1,470 | 1,470 | +0.0% |
| search-signal: p50 (ms) | 0.432 | 0.433 | +0.2% |
| search-signal: p95 (ms) | 1.027 | 0.673 | -34.5% |
| search-signal: peak RSS (MiB) | 5.18 | 5.12 | -1.2% |
| find-signal: probes | 180 | 180 | +0.0% |
| find-signal: matches | 267,480 | 267,480 | +0.0% |
| find-signal: p50 (ms) | 0.462 | 0.448 | -3.0% |
| find-signal: p95 (ms) | 0.726 | 0.625 | -13.9% |
| find-signal: peak RSS (MiB) | 5.18 | 5.12 | -1.2% |
| point: probes | 180 | 180 | +0.0% |
| point: matches | 180 | 180 | +0.0% |
| point: p50 (ms) | 0.429 | 0.432 | +0.7% |
| point: p95 (ms) | 0.696 | 0.553 | -20.5% |
| point: peak RSS (MiB) | 5.26 | 5.21 | -1.0% |
| search-transaction: probes | 180 | 180 | +0.0% |
| search-transaction: matches | 120 | 120 | +0.0% |
| search-transaction: p50 (ms) | 0.346 | 0.343 | -0.9% |
| search-transaction: p95 (ms) | 0.469 | 0.436 | -7.0% |
| search-transaction: peak RSS (MiB) | 4.89 | 4.83 | -1.2% |
| find-transaction: probes | 180 | 180 | +0.0% |
| find-transaction: matches | 2,715 | 2,715 | +0.0% |
| find-transaction: p50 (ms) | 1.750 | 1.734 | -0.9% |
| find-transaction: p95 (ms) | 2.055 | 2.013 | -2.0% |
| find-transaction: peak RSS (MiB) | 4.89 | 4.83 | -1.2% |
