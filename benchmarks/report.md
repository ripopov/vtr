# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **67.24 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 43.14 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 109.79 | 420.71 | 357.24 |
| Write throughput (Mobs/s) | 57.300 | 14.954 | 17.610 |
| Read time (ms) | 46.73 | 138.22 | 213.95 |
| Read throughput (Mobs/s) | 134.612 | 45.516 | 29.404 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 21.67 | 143.25 | 4.66 |
| Read peak RSS (MiB) | 12.80 | 25.74 | 5.73 |
| Capture time (ms) | 97.89 | 361.70 | 350.80 |
| Capture time (ns/tick) | 203.9 | 753.5 | 730.8 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 11.72 | 58.64 | 5.83 |
| Write CPU time (ms) | 97.53 | 405.63 | 150.05 |
| Write min / max (ms) | 108.05 / 112.74 | 419.77 / 429.88 | 246.73 / 379.27 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.555 | 1.296 | 76.731 |
| search-signal: p95 (ms) | 0.848 | 5.046 | 122.459 |
| search-signal: peak RSS (MiB) | 7.20 | 27.47 | 6.73 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 1.739 | 1.707 | 130.291 |
| find-signal: p95 (ms) | 4.218 | 6.672 | 166.615 |
| find-signal: peak RSS (MiB) | 8.47 | 27.27 | 6.76 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.508 | 0.502 | 78.114 |
| point: p95 (ms) | 0.781 | 1.096 | 124.974 |
| point: peak RSS (MiB) | 7.79 | 9.57 | 7.31 |

### soc

Observations: 20,143,793. Null producer: 115.40 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 454.24 | 943.31 | 1047.53 |
| Write throughput (Mobs/s) | 44.346 | 21.354 | 19.230 |
| Read time (ms) | 147.59 | 470.09 | 679.65 |
| Read throughput (Mobs/s) | 136.485 | 42.851 | 29.638 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 17.32 | 147.26 | 5.73 |
| Read peak RSS (MiB) | 30.66 | 23.46 | 4.65 |
| Capture time (ms) | 434.25 | 858.16 | 1038.30 |
| Capture time (ns/tick) | 18093.6 | 35756.7 | 43262.5 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 17.94 | 81.38 | 5.88 |
| Write CPU time (ms) | 408.82 | 717.85 | 437.52 |
| Write min / max (ms) | 418.87 / 459.05 | 938.67 / 955.08 | 987.74 / 1050.63 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.994 | 0.518 | 249.214 |
| search-signal: p95 (ms) | 3.026 | 1.249 | 355.942 |
| search-signal: peak RSS (MiB) | 8.08 | 23.12 | 7.24 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 1.964 | 1.066 | 405.573 |
| find-signal: p95 (ms) | 2.414 | 1.489 | 426.092 |
| find-signal: peak RSS (MiB) | 9.31 | 27.01 | 7.40 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.931 | 0.388 | 247.999 |
| point: p95 (ms) | 1.311 | 0.481 | 348.382 |
| point: peak RSS (MiB) | 8.11 | 4.14 | 6.91 |

### idle

Observations: 2,547,072. Null producer: 15.62 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 57.71 | 108.52 | 133.84 |
| Write throughput (Mobs/s) | 44.136 | 23.470 | 19.031 |
| Read time (ms) | 24.19 | 58.45 | 82.43 |
| Read throughput (Mobs/s) | 105.278 | 43.580 | 30.902 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 16.63 | 78.86 | 4.90 |
| Read peak RSS (MiB) | 8.09 | 15.00 | 4.64 |
| Capture time (ms) | 46.75 | 30.29 | 125.65 |
| Capture time (ns/tick) | 973.9 | 631.1 | 2617.7 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 10.16 | 77.04 | 6.46 |
| Write CPU time (ms) | 53.70 | 82.27 | 58.58 |
| Write min / max (ms) | 56.06 / 61.43 | 105.14 / 109.28 | 130.21 / 135.98 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.530 | 0.310 | 19.079 |
| search-signal: p95 (ms) | 0.788 | 0.483 | 46.940 |
| search-signal: peak RSS (MiB) | 6.79 | 15.75 | 6.59 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.596 | 0.292 | 49.127 |
| find-signal: p95 (ms) | 0.802 | 0.423 | 50.990 |
| find-signal: peak RSS (MiB) | 7.83 | 15.62 | 7.01 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.509 | 0.250 | 18.897 |
| point: p95 (ms) | 0.696 | 0.382 | 47.301 |
| point: peak RSS (MiB) | 7.53 | 5.88 | 7.05 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.90 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 35.87 | 42.36 |
| Write throughput (Mobs/s) | 5.736 | 4.856 |
| Read time (ms) | 17.51 | 12.92 |
| Read throughput (Mobs/s) | 11.748 | 15.927 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 7.24 | 4.12 |
| Read peak RSS (MiB) | 7.20 | 7.70 |
| Capture time (ms) | 32.50 | 36.09 |
| Capture time (ns/tick) | 270.8 | 300.8 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 3.35 | 4.76 |
| Write CPU time (ms) | 28.92 | 33.27 |
| Write min / max (ms) | 35.83 / 37.49 | 40.16 / 43.40 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.571 | 4.967 |
| search-transaction: p95 (ms) | 0.678 | 5.889 |
| search-transaction: peak RSS (MiB) | 7.24 | 7.63 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 4.542 | 6.457 |
| find-transaction: p95 (ms) | 4.899 | 6.681 |
| find-transaction: peak RSS (MiB) | 7.34 | 9.62 |

### many-generators

Observations: 329,168. Null producer: 16.39 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 60.69 | 75.88 |
| Write throughput (Mobs/s) | 5.424 | 4.338 |
| Read time (ms) | 27.01 | 24.75 |
| Read throughput (Mobs/s) | 12.187 | 13.299 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 5.71 | 6.03 |
| Read peak RSS (MiB) | 7.76 | 10.58 |
| Capture time (ms) | 57.15 | 65.82 |
| Capture time (ns/tick) | 2381.3 | 2742.6 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 3.14 | 9.78 |
| Write CPU time (ms) | 51.00 | 61.44 |
| Write min / max (ms) | 60.59 / 61.59 | 75.75 / 82.46 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.930 | 3.382 |
| search-transaction: p95 (ms) | 1.085 | 6.766 |
| search-transaction: peak RSS (MiB) | 5.96 | 10.46 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 5.540 | 3.898 |
| find-transaction: p95 (ms) | 16.055 | 4.473 |
| find-transaction: peak RSS (MiB) | 6.03 | 10.46 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.84 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 39.73 | 74.01 | 79.75 |
| Write throughput (Mobs/s) | 33.761 | 18.122 | 16.819 |
| Read time (ms) | 16.07 | 36.25 | 51.72 |
| Read throughput (Mobs/s) | 83.478 | 37.004 | 25.934 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 17.10 | 43.18 | 5.29 |
| Read peak RSS (MiB) | 6.50 | 10.10 | 6.00 |
| Capture time (ms) | 32.02 | 31.15 | 74.41 |
| Capture time (ns/tick) | 1334.2 | 1297.7 | 3100.4 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 5.47 | 39.29 | 4.91 |
| Write CPU time (ms) | 33.47 | 62.56 | 46.50 |
| Write min / max (ms) | 37.91 / 48.76 | 65.66 / 95.83 | 55.94 / 88.87 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.405 | 0.273 | 19.428 |
| search-signal: p95 (ms) | 0.527 | 0.551 | 28.276 |
| search-signal: peak RSS (MiB) | 7.02 | 10.38 | 7.39 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.422 | 0.259 | 28.066 |
| find-signal: p95 (ms) | 0.549 | 0.522 | 29.006 |
| find-signal: peak RSS (MiB) | 7.01 | 10.40 | 7.21 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.391 | 0.155 | 18.664 |
| point: p95 (ms) | 0.531 | 0.238 | 26.550 |
| point: peak RSS (MiB) | 5.18 | 4.18 | 7.25 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.339 | 1.002 | 1.005 |
| search-transaction: p95 (ms) | 0.407 | 1.679 | 1.587 |
| search-transaction: peak RSS (MiB) | 4.89 | 5.00 | 7.00 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.737 | 1.601 | 1.609 |
| find-transaction: p95 (ms) | 1.954 | 1.799 | 1.678 |
| find-transaction: peak RSS (MiB) | 4.88 | 5.00 | 5.00 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
