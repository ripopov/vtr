# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **54.16 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 49.32 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 112.45 | 694.68 | 168.25 |
| Write throughput (Mobs/s) | 55.946 | 9.056 | 37.392 |
| Read time (ms) | 41.01 | 170.31 | 181.89 |
| Read throughput (Mobs/s) | 153.394 | 36.939 | 34.587 |
| File size (MiB) | 7.666 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.91 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 51.70 | 140.89 | 5.39 |
| Read peak RSS (MiB) | 17.73 | 26.25 | 5.07 |
| Capture time (ms) | 87.55 | 576.24 | 167.61 |
| Capture time (ns/tick) | 182.4 | 1200.5 | 349.2 |
| File bytes per tick | 16.7 | 19.5 | 326.2 |
| Close time (ms) | 24.59 | 117.58 | 0.13 |
| Write CPU time (ms) | 113.47 | 695.52 | 168.35 |
| Write min / max (ms) | 111.46 / 112.61 | 692.86 / 697.73 | 166.37 / 170.88 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.445 | 2.530 | 56.492 |
| search-signal: p95 (ms) | 1.907 | 6.264 | 89.290 |
| search-signal: peak RSS (MiB) | 12.62 | 26.88 | 6.55 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.707 | 3.161 | 93.196 |
| find-signal: p95 (ms) | 6.173 | 8.543 | 97.175 |
| find-signal: peak RSS (MiB) | 12.70 | 26.84 | 6.54 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.518 | 0.360 | 55.738 |
| point: p95 (ms) | 2.300 | 1.570 | 90.313 |
| point: peak RSS (MiB) | 12.66 | 8.84 | 6.57 |

### soc

Observations: 20,143,793. Null producer: 143.56 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 363.20 | 854.38 | 512.98 |
| Write throughput (Mobs/s) | 55.462 | 23.577 | 39.268 |
| Read time (ms) | 106.72 | 592.83 | 574.60 |
| Read throughput (Mobs/s) | 188.753 | 33.979 | 35.057 |
| File size (MiB) | 24.806 | 52.345 | 485.555 |
| Bytes per observation | 1.29 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.78 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 114.29 | 145.45 | 5.59 |
| Read peak RSS (MiB) | 30.14 | 22.54 | 5.25 |
| Capture time (ms) | 346.46 | 829.34 | 512.06 |
| Capture time (ns/tick) | 14435.9 | 34555.7 | 21336.0 |
| File bytes per tick | 1083.8 | 2287.0 | 21214.2 |
| Close time (ms) | 15.61 | 24.42 | 0.16 |
| Write CPU time (ms) | 364.24 | 850.68 | 513.91 |
| Write min / max (ms) | 362.79 / 369.16 | 849.79 / 858.95 | 511.37 / 516.90 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.279 | 0.432 | 184.989 |
| search-signal: p95 (ms) | 0.475 | 6.770 | 262.424 |
| search-signal: peak RSS (MiB) | 6.35 | 22.59 | 6.55 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.318 | 7.358 | 281.992 |
| find-signal: p95 (ms) | 0.597 | 7.656 | 284.761 |
| find-signal: peak RSS (MiB) | 6.66 | 22.32 | 6.54 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.291 | 0.197 | 183.031 |
| point: p95 (ms) | 0.464 | 0.314 | 259.980 |
| point: peak RSS (MiB) | 6.41 | 4.96 | 6.51 |

### idle

Observations: 2,547,072. Null producer: 17.99 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 50.25 | 117.10 | 65.84 |
| Write throughput (Mobs/s) | 50.693 | 21.752 | 38.685 |
| Read time (ms) | 15.64 | 74.99 | 72.82 |
| Read throughput (Mobs/s) | 162.868 | 33.964 | 34.978 |
| File size (MiB) | 1.853 | 4.165 | 61.551 |
| Bytes per observation | 0.76 | 1.71 | 25.34 |
| Logical bytes / file bytes | 19.95 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 22.11 | 77.48 | 5.61 |
| Read peak RSS (MiB) | 7.09 | 14.62 | 5.25 |
| Capture time (ms) | 36.47 | 59.90 | 64.95 |
| Capture time (ns/tick) | 759.7 | 1247.8 | 1353.1 |
| File bytes per tick | 40.5 | 91.0 | 1344.6 |
| Close time (ms) | 12.54 | 56.60 | 0.17 |
| Write CPU time (ms) | 51.10 | 117.89 | 66.73 |
| Write min / max (ms) | 49.72 / 50.50 | 116.94 / 117.93 | 65.45 / 66.05 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.328 | 0.335 | 13.796 |
| search-signal: p95 (ms) | 0.522 | 4.258 | 33.554 |
| search-signal: peak RSS (MiB) | 5.95 | 14.96 | 6.55 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.336 | 0.333 | 35.094 |
| find-signal: p95 (ms) | 0.523 | 4.213 | 35.723 |
| find-signal: peak RSS (MiB) | 5.88 | 14.98 | 6.53 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.334 | 0.097 | 13.545 |
| point: p95 (ms) | 0.509 | 0.329 | 34.438 |
| point: peak RSS (MiB) | 5.82 | 5.41 | 6.56 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.28 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 30.24 | 37.92 |
| Write throughput (Mobs/s) | 6.804 | 5.426 |
| Read time (ms) | 16.66 | 11.81 |
| Read throughput (Mobs/s) | 12.350 | 17.424 |
| File size (MiB) | 2.292 | 4.424 |
| Bytes per observation | 11.68 | 22.55 |
| Logical bytes / file bytes | 5.82 | 3.02 |
| Write peak RSS (MiB) | 9.70 | 4.86 |
| Read peak RSS (MiB) | 7.77 | 8.72 |
| Capture time (ms) | 29.12 | 37.69 |
| Capture time (ns/tick) | 242.7 | 314.1 |
| File bytes per tick | 20.0 | 38.7 |
| Close time (ms) | 1.02 | 0.17 |
| Write CPU time (ms) | 31.19 | 38.73 |
| Write min / max (ms) | 29.98 / 30.29 | 36.86 / 38.07 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 1.884 | 3.648 |
| search-transaction: p95 (ms) | 2.350 | 4.462 |
| search-transaction: peak RSS (MiB) | 6.11 | 8.62 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 10.824 | 5.043 |
| find-transaction: p95 (ms) | 11.781 | 5.367 |
| find-transaction: peak RSS (MiB) | 7.74 | 8.62 |

### many-generators

Observations: 329,168. Null producer: 13.89 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 56.07 | 67.45 |
| Write throughput (Mobs/s) | 5.870 | 4.880 |
| Read time (ms) | 29.16 | 20.96 |
| Read throughput (Mobs/s) | 11.290 | 15.708 |
| File size (MiB) | 3.775 | 7.571 |
| Bytes per observation | 12.02 | 24.12 |
| Logical bytes / file bytes | 5.66 | 2.82 |
| Write peak RSS (MiB) | 9.34 | 6.08 |
| Read peak RSS (MiB) | 9.17 | 11.90 |
| Capture time (ms) | 54.69 | 66.69 |
| Capture time (ns/tick) | 2278.9 | 2778.7 |
| File bytes per tick | 164.9 | 330.8 |
| Close time (ms) | 1.28 | 0.70 |
| Write CPU time (ms) | 57.04 | 68.32 |
| Write min / max (ms) | 55.31 / 56.88 | 66.52 / 69.47 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 1.926 | 0.617 |
| search-transaction: p95 (ms) | 3.764 | 1.365 |
| search-transaction: peak RSS (MiB) | 6.80 | 11.87 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 16.694 | 1.343 |
| find-transaction: p95 (ms) | 17.877 | 1.482 |
| find-transaction: peak RSS (MiB) | 9.23 | 11.83 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 12.14 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 35.84 | 93.30 | 49.37 |
| Write throughput (Mobs/s) | 37.420 | 14.376 | 27.169 |
| Read time (ms) | 14.79 | 39.32 | 41.15 |
| Read throughput (Mobs/s) | 90.694 | 34.112 | 32.595 |
| File size (MiB) | 2.500 | 4.010 | 31.812 |
| Bytes per observation | 1.95 | 3.13 | 24.87 |
| Logical bytes / file bytes | 9.44 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 17.30 | 40.98 | 6.03 |
| Read peak RSS (MiB) | 8.21 | 9.28 | 6.09 |
| Capture time (ms) | 31.12 | 46.06 | 48.57 |
| Capture time (ns/tick) | 1296.9 | 1919.2 | 2024.0 |
| File bytes per tick | 109.2 | 175.2 | 1389.9 |
| Close time (ms) | 4.62 | 47.05 | 0.27 |
| Write CPU time (ms) | 36.77 | 94.23 | 50.30 |
| Write min / max (ms) | 35.81 / 36.05 | 92.52 / 94.56 | 48.98 / 49.90 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.054 | 0.238 | 11.884 |
| search-signal: p95 (ms) | 0.190 | 2.056 | 17.092 |
| search-signal: peak RSS (MiB) | 6.03 | 9.74 | 6.56 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.065 | 0.228 | 17.850 |
| find-signal: p95 (ms) | 0.339 | 1.988 | 18.542 |
| find-signal: peak RSS (MiB) | 5.99 | 9.70 | 6.56 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.058 | 0.039 | 11.957 |
| point: p95 (ms) | 0.198 | 0.165 | 16.797 |
| point: peak RSS (MiB) | 5.95 | 5.05 | 6.57 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 2.048 | 0.432 | 0.431 |
| search-transaction: p95 (ms) | 2.719 | 1.123 | 1.096 |
| search-transaction: peak RSS (MiB) | 6.54 | 6.07 | 6.00 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 4.541 | 1.096 | 1.089 |
| find-transaction: p95 (ms) | 5.658 | 1.186 | 1.173 |
| find-transaction: peak RSS (MiB) | 6.54 | 6.04 | 6.05 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. VTR uses its default reader mode without whole-section CRC scans; validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
