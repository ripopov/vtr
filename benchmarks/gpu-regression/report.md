# Native VTR paired comparison

Status: **complete; all checks passed**. Measured 2026-11-11T17:44:39.542609+00:00. Elapsed: 31.49 s.

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Paired repetitions: 3. Full default workload sizes.

Each pair runs both binaries in shuffled order on one CPU. Each binary writes its own trace, then runs the full read and all twelve probes of every applicable query. Every result is validated against both binaries' independent producer replays. Timings include complete writes, full reads, and fresh-reader queries; lower time and RSS are better. Changes below are candidate / baseline − 1. Small differences and process RSS variation are not evidence of a speedup or regression.

Both executables use their own source replay and files. Preserve build provenance and compare every metric, including slower results. See the benchmark guide for the shared workload and timing boundaries.

Baseline: b1a68f3. Binary SHA-256: `5c340bbc8fb8eb7143073c065bd8be0d4b2b17cb58030779d66eaa61f7059cd9`.

Candidate: separate-relation-pages. Binary SHA-256: `b9a9ffb2057212a13d08e6ac1a5e5cbd3207f710ce5dc1cf01993040684bd283`.

## core

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 98.39 | 97.04 | -1.4% |
| Write throughput (Mobs/s) | 63.938 | 64.829 | +1.4% |
| File size (MiB) | 7.695 | 7.695 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.86 | 11.86 | +0.0% |
| Write peak RSS (MiB) | 21.33 | 21.63 | +1.4% |
| Capture time (ms) | 96.61 | 95.94 | -0.7% |
| Capture time (ns/tick) | 201.3 | 199.9 | -0.7% |
| File bytes per tick | 16.8 | 16.8 | +0.0% |
| Close time (ms) | 1.15 | 1.10 | -4.3% |
| Write CPU time (ms) | 98.87 | 97.53 | -1.4% |
| Write min / max (ms) | 96.92 / 99.58 | 94.98 / 103.26 | — |
| Read time (ms) | 43.02 | 43.96 | +2.2% |
| Read throughput (Mobs/s) | 146.241 | 143.118 | -2.1% |
| Read peak RSS (MiB) | 13.93 | 14.37 | +3.2% |
| search-signal: probes | 36 | 36 | +0.0% |
| search-signal: matches | 291 | 291 | +0.0% |
| search-signal: p50 (ms) | 0.250 | 0.244 | -2.4% |
| search-signal: p95 (ms) | 0.410 | 0.426 | +3.9% |
| search-signal: peak RSS (MiB) | 8.84 | 8.56 | -3.2% |
| find-signal: probes | 36 | 36 | +0.0% |
| find-signal: matches | 1,068,627 | 1,068,627 | +0.0% |
| find-signal: p50 (ms) | 1.394 | 1.240 | -11.0% |
| find-signal: p95 (ms) | 3.873 | 3.626 | -6.4% |
| find-signal: peak RSS (MiB) | 11.22 | 9.80 | -12.7% |
| point: probes | 36 | 36 | +0.0% |
| point: matches | 36 | 36 | +0.0% |
| point: p50 (ms) | 0.260 | 0.254 | -2.3% |
| point: p95 (ms) | 0.563 | 0.569 | +1.1% |
| point: peak RSS (MiB) | 8.23 | 8.41 | +2.2% |

## soc

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 372.80 | 375.56 | +0.7% |
| Write throughput (Mobs/s) | 54.033 | 53.637 | -0.7% |
| File size (MiB) | 24.579 | 24.579 | +0.0% |
| Bytes per observation | 1.28 | 1.28 | +0.0% |
| Logical bytes / file bytes | 11.89 | 11.89 | +0.0% |
| Write peak RSS (MiB) | 17.77 | 17.33 | -2.5% |
| Capture time (ms) | 366.07 | 369.19 | +0.9% |
| Capture time (ns/tick) | 15253.0 | 15382.8 | +0.9% |
| File bytes per tick | 1073.9 | 1073.9 | +0.0% |
| Close time (ms) | 5.75 | 6.27 | +9.0% |
| Write CPU time (ms) | 373.21 | 375.95 | +0.7% |
| Write min / max (ms) | 371.33 / 392.30 | 369.05 / 419.24 | — |
| Read time (ms) | 134.86 | 133.68 | -0.9% |
| Read throughput (Mobs/s) | 149.368 | 150.681 | +0.9% |
| Read peak RSS (MiB) | 31.74 | 31.74 | +0.0% |
| search-signal: probes | 36 | 36 | +0.0% |
| search-signal: matches | 297 | 297 | +0.0% |
| search-signal: p50 (ms) | 0.291 | 0.292 | +0.3% |
| search-signal: p95 (ms) | 0.483 | 0.453 | -6.2% |
| search-signal: peak RSS (MiB) | 8.37 | 8.63 | +3.1% |
| find-signal: probes | 36 | 36 | +0.0% |
| find-signal: matches | 53,517 | 53,517 | +0.0% |
| find-signal: p50 (ms) | 0.430 | 0.426 | -0.9% |
| find-signal: p95 (ms) | 0.612 | 0.605 | -1.1% |
| find-signal: peak RSS (MiB) | 11.36 | 11.54 | +1.6% |
| point: probes | 36 | 36 | +0.0% |
| point: matches | 36 | 36 | +0.0% |
| point: p50 (ms) | 0.276 | 0.288 | +4.3% |
| point: p95 (ms) | 0.440 | 0.429 | -2.5% |
| point: peak RSS (MiB) | 8.90 | 8.28 | -7.0% |

## idle

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 53.70 | 51.26 | -4.5% |
| Write throughput (Mobs/s) | 47.434 | 49.690 | +4.8% |
| File size (MiB) | 1.732 | 1.732 | +0.0% |
| Bytes per observation | 0.71 | 0.71 | +0.0% |
| Logical bytes / file bytes | 21.34 | 21.34 | +0.0% |
| Write peak RSS (MiB) | 15.61 | 15.68 | +0.4% |
| Capture time (ms) | 46.11 | 43.97 | -4.6% |
| Capture time (ns/tick) | 960.7 | 916.1 | -4.6% |
| File bytes per tick | 37.8 | 37.8 | +0.0% |
| Close time (ms) | 6.89 | 6.54 | -5.1% |
| Write CPU time (ms) | 54.08 | 51.67 | -4.5% |
| Write min / max (ms) | 51.88 / 56.07 | 51.04 / 53.17 | — |
| Read time (ms) | 17.80 | 17.76 | -0.2% |
| Read throughput (Mobs/s) | 143.128 | 143.389 | +0.2% |
| Read peak RSS (MiB) | 8.85 | 9.00 | +1.7% |
| search-signal: probes | 36 | 36 | +0.0% |
| search-signal: matches | 3 | 3 | +0.0% |
| search-signal: p50 (ms) | 0.314 | 0.324 | +3.2% |
| search-signal: p95 (ms) | 0.516 | 0.508 | -1.6% |
| search-signal: peak RSS (MiB) | 7.51 | 7.73 | +2.9% |
| find-signal: probes | 36 | 36 | +0.0% |
| find-signal: matches | 6,744 | 6,744 | +0.0% |
| find-signal: p50 (ms) | 0.369 | 0.363 | -1.6% |
| find-signal: p95 (ms) | 0.525 | 0.543 | +3.4% |
| find-signal: peak RSS (MiB) | 7.83 | 7.82 | -0.1% |
| point: probes | 36 | 36 | +0.0% |
| point: matches | 36 | 36 | +0.0% |
| point: p50 (ms) | 0.318 | 0.314 | -1.3% |
| point: p95 (ms) | 0.475 | 0.486 | +2.3% |
| point: peak RSS (MiB) | 7.41 | 7.40 | -0.1% |

## requests

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 27.01 | 28.91 | +7.0% |
| Write throughput (Mobs/s) | 7.618 | 7.117 | -6.6% |
| File size (MiB) | 1.046 | 1.046 | +0.0% |
| Bytes per observation | 5.33 | 5.33 | +0.0% |
| Logical bytes / file bytes | 12.75 | 12.75 | +0.0% |
| Write peak RSS (MiB) | 5.71 | 6.69 | +17.2% |
| Capture time (ms) | 26.81 | 28.76 | +7.3% |
| Capture time (ns/tick) | 223.4 | 239.7 | +7.3% |
| File bytes per tick | 9.1 | 9.1 | +0.0% |
| Close time (ms) | 0.11 | 0.09 | -18.2% |
| Write CPU time (ms) | 27.39 | 29.32 | +7.0% |
| Write min / max (ms) | 26.88 / 27.52 | 27.14 / 29.41 | — |
| Read time (ms) | 14.40 | 14.30 | -0.7% |
| Read throughput (Mobs/s) | 14.289 | 14.385 | +0.7% |
| Read peak RSS (MiB) | 7.30 | 5.32 | -27.1% |
| search-transaction: probes | 36 | 36 | +0.0% |
| search-transaction: matches | 27 | 27 | +0.0% |
| search-transaction: p50 (ms) | 0.086 | 0.085 | -1.2% |
| search-transaction: p95 (ms) | 0.127 | 0.130 | +2.4% |
| search-transaction: peak RSS (MiB) | 5.32 | 7.41 | +39.3% |
| find-transaction: probes | 36 | 36 | +0.0% |
| find-transaction: matches | 2,715 | 2,715 | +0.0% |
| find-transaction: p50 (ms) | 3.803 | 4.002 | +5.2% |
| find-transaction: p95 (ms) | 3.991 | 4.266 | +6.9% |
| find-transaction: peak RSS (MiB) | 5.32 | 7.30 | +37.2% |

## many-generators

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 48.92 | 48.89 | -0.1% |
| Write throughput (Mobs/s) | 6.728 | 6.733 | +0.1% |
| File size (MiB) | 1.662 | 1.662 | +0.0% |
| Bytes per observation | 5.29 | 5.29 | +0.0% |
| Logical bytes / file bytes | 12.84 | 12.84 | +0.0% |
| Write peak RSS (MiB) | 6.79 | 6.93 | +2.1% |
| Capture time (ms) | 48.71 | 48.67 | -0.1% |
| Capture time (ns/tick) | 2029.4 | 2028.0 | -0.1% |
| File bytes per tick | 72.6 | 72.6 | +0.0% |
| Close time (ms) | 0.12 | 0.11 | -8.3% |
| Write CPU time (ms) | 49.36 | 49.30 | -0.1% |
| Write min / max (ms) | 48.54 / 50.98 | 48.76 / 50.23 | — |
| Read time (ms) | 25.25 | 25.32 | +0.3% |
| Read throughput (Mobs/s) | 13.037 | 12.999 | -0.3% |
| Read peak RSS (MiB) | 6.06 | 8.11 | +33.8% |
| search-transaction: probes | 36 | 36 | +0.0% |
| search-transaction: matches | 15 | 15 | +0.0% |
| search-transaction: p50 (ms) | 0.188 | 0.187 | -0.5% |
| search-transaction: p95 (ms) | 0.252 | 0.250 | -0.8% |
| search-transaction: peak RSS (MiB) | 6.06 | 6.06 | +0.0% |
| find-transaction: probes | 36 | 36 | +0.0% |
| find-transaction: matches | 537 | 537 | +0.0% |
| find-transaction: p50 (ms) | 4.558 | 4.506 | -1.1% |
| find-transaction: p95 (ms) | 4.758 | 4.839 | +1.7% |
| find-transaction: peak RSS (MiB) | 7.92 | 6.06 | -23.5% |

## mixed

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 31.67 | 30.85 | -2.6% |
| Write throughput (Mobs/s) | 42.349 | 43.480 | +2.7% |
| File size (MiB) | 1.970 | 1.970 | +0.0% |
| Bytes per observation | 1.54 | 1.54 | +0.0% |
| Logical bytes / file bytes | 11.98 | 11.98 | +0.0% |
| Write peak RSS (MiB) | 16.38 | 17.09 | +4.3% |
| Capture time (ms) | 29.08 | 28.11 | -3.3% |
| Capture time (ns/tick) | 1211.6 | 1171.4 | -3.3% |
| File bytes per tick | 86.1 | 86.1 | +0.0% |
| Close time (ms) | 2.51 | 2.52 | +0.4% |
| Write CPU time (ms) | 32.08 | 31.24 | -2.6% |
| Write min / max (ms) | 31.40 / 32.07 | 30.77 / 31.26 | — |
| Read time (ms) | 14.74 | 14.39 | -2.4% |
| Read throughput (Mobs/s) | 91.024 | 93.195 | +2.4% |
| Read peak RSS (MiB) | 6.55 | 6.55 | +0.0% |
| search-signal: probes | 36 | 36 | +0.0% |
| search-signal: matches | 294 | 294 | +0.0% |
| search-signal: p50 (ms) | 0.109 | 0.111 | +1.8% |
| search-signal: p95 (ms) | 0.182 | 0.199 | +9.3% |
| search-signal: peak RSS (MiB) | 7.33 | 7.64 | +4.2% |
| find-signal: probes | 36 | 36 | +0.0% |
| find-signal: matches | 53,496 | 53,496 | +0.0% |
| find-signal: p50 (ms) | 0.123 | 0.123 | +0.0% |
| find-signal: p95 (ms) | 0.247 | 0.254 | +2.8% |
| find-signal: peak RSS (MiB) | 7.33 | 7.64 | +4.2% |
| point: probes | 36 | 36 | +0.0% |
| point: matches | 36 | 36 | +0.0% |
| point: p50 (ms) | 0.109 | 0.120 | +10.1% |
| point: p95 (ms) | 0.194 | 0.161 | -17.0% |
| point: peak RSS (MiB) | 5.68 | 7.30 | +28.5% |
| search-transaction: probes | 36 | 36 | +0.0% |
| search-transaction: matches | 24 | 24 | +0.0% |
| search-transaction: p50 (ms) | 0.079 | 0.078 | -1.3% |
| search-transaction: p95 (ms) | 0.116 | 0.123 | +6.0% |
| search-transaction: peak RSS (MiB) | 4.99 | 4.90 | -1.8% |
| find-transaction: probes | 36 | 36 | +0.0% |
| find-transaction: matches | 543 | 543 | +0.0% |
| find-transaction: p50 (ms) | 1.403 | 1.420 | +1.2% |
| find-transaction: p95 (ms) | 1.487 | 1.590 | +6.9% |
| find-transaction: peak RSS (MiB) | 4.99 | 6.98 | +39.9% |

## gpu-instructions

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 132.23 | 91.58 | -30.7% |
| Write throughput (Mobs/s) | 1.983 | 2.862 | +44.3% |
| File size (MiB) | 3.713 | 3.754 | +1.1% |
| Bytes per observation | 14.85 | 15.02 | +1.1% |
| Logical bytes / file bytes | 18.41 | 18.21 | -1.1% |
| Write peak RSS (MiB) | 7.13 | 8.00 | +12.2% |
| Capture time (ms) | 131.84 | 91.26 | -30.8% |
| Capture time (ns/tick) | 502.9 | 348.1 | -30.8% |
| File bytes per tick | 14.9 | 15.0 | +0.7% |
| Close time (ms) | 0.19 | 0.13 | -31.6% |
| Write CPU time (ms) | 132.70 | 92.08 | -30.6% |
| Write min / max (ms) | 91.70 / 136.75 | 90.77 / 125.89 | — |
| Transaction blocks | 324 | 324 | +0.0% |
| Peak transaction buffer (KiB) | 512.60 | 512.60 | +0.0% |
| Read time (ms) | 2660.93 | 529.15 | -80.1% |
| Read throughput (Mobs/s) | 0.099 | 0.495 | +400.0% |
| Read peak RSS (MiB) | 10.64 | 10.47 | -1.6% |

## gpu-instructions-zstd

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Write time (ms) | 109.34 | 102.92 | -5.9% |
| Write throughput (Mobs/s) | 2.398 | 2.547 | +6.2% |
| File size (MiB) | 2.391 | 2.313 | -3.3% |
| Bytes per observation | 9.57 | 9.25 | -3.3% |
| Logical bytes / file bytes | 28.58 | 29.55 | +3.4% |
| Write peak RSS (MiB) | 8.18 | 8.10 | -1.0% |
| Capture time (ms) | 108.93 | 102.54 | -5.9% |
| Capture time (ns/tick) | 415.5 | 391.2 | -5.8% |
| File bytes per tick | 9.6 | 9.3 | -3.1% |
| Close time (ms) | 0.16 | 0.15 | -6.2% |
| Write CPU time (ms) | 109.98 | 103.48 | -5.9% |
| Write min / max (ms) | 102.74 / 132.47 | 97.38 / 140.14 | — |
| Transaction blocks | 324 | 324 | +0.0% |
| Peak transaction buffer (KiB) | 512.60 | 512.60 | +0.0% |
| Read time (ms) | 3023.95 | 729.89 | -75.9% |
| Read throughput (Mobs/s) | 0.087 | 0.359 | +312.6% |
| Read peak RSS (MiB) | 7.64 | 8.98 | +17.5% |
