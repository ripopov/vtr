# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **63.79 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 42.51 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 102.77 | 430.60 | 201.11 |
| Write throughput (Mobs/s) | 61.214 | 14.610 | 31.281 |
| Read time (ms) | 46.54 | 138.93 | 209.06 |
| Read throughput (Mobs/s) | 135.170 | 45.284 | 30.093 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 21.61 | 143.14 | 4.63 |
| Read peak RSS (MiB) | 12.84 | 25.68 | 5.62 |
| Capture time (ms) | 100.72 | 380.73 | 200.23 |
| Capture time (ns/tick) | 209.8 | 793.2 | 417.1 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.66 | 48.39 | 0.64 |
| Write CPU time (ms) | 100.52 | 422.22 | 162.59 |
| Write min / max (ms) | 101.41 / 104.99 | 412.65 / 437.13 | 194.25 / 205.80 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.563 | 1.356 | 78.214 |
| search-signal: p95 (ms) | 0.988 | 5.068 | 122.658 |
| search-signal: peak RSS (MiB) | 7.66 | 26.39 | 6.86 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 1.723 | 1.787 | 127.772 |
| find-signal: p95 (ms) | 4.147 | 6.714 | 132.906 |
| find-signal: peak RSS (MiB) | 8.04 | 27.65 | 6.73 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.577 | 0.517 | 77.542 |
| point: p95 (ms) | 1.003 | 1.231 | 122.491 |
| point: peak RSS (MiB) | 7.18 | 8.98 | 6.60 |

### soc

Observations: 20,143,793. Null producer: 114.98 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 412.01 | 924.00 | 700.70 |
| Write throughput (Mobs/s) | 48.892 | 21.801 | 28.748 |
| Read time (ms) | 146.40 | 473.87 | 651.92 |
| Read throughput (Mobs/s) | 137.597 | 42.509 | 30.899 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 17.64 | 147.49 | 4.89 |
| Read peak RSS (MiB) | 30.91 | 22.97 | 5.61 |
| Capture time (ms) | 403.46 | 885.55 | 699.36 |
| Capture time (ns/tick) | 16810.9 | 36898.0 | 29140.0 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 7.30 | 35.03 | 0.74 |
| Write CPU time (ms) | 401.13 | 752.91 | 464.67 |
| Write min / max (ms) | 407.12 / 448.40 | 916.72 / 925.71 | 624.05 / 719.09 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.996 | 0.562 | 246.527 |
| search-signal: p95 (ms) | 3.273 | 0.971 | 348.208 |
| search-signal: peak RSS (MiB) | 8.19 | 23.56 | 6.33 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 2.098 | 1.131 | 373.166 |
| find-signal: p95 (ms) | 2.792 | 1.475 | 381.659 |
| find-signal: peak RSS (MiB) | 10.01 | 26.99 | 7.18 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.964 | 0.395 | 243.265 |
| point: p95 (ms) | 1.473 | 0.509 | 345.542 |
| point: peak RSS (MiB) | 7.86 | 5.89 | 6.70 |

### idle

Observations: 2,547,072. Null producer: 14.98 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 56.65 | 105.83 | 78.60 |
| Write throughput (Mobs/s) | 44.962 | 24.067 | 32.406 |
| Read time (ms) | 19.42 | 59.21 | 82.30 |
| Read throughput (Mobs/s) | 131.144 | 43.018 | 30.947 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 16.33 | 79.09 | 5.63 |
| Read peak RSS (MiB) | 8.20 | 13.79 | 5.59 |
| Capture time (ms) | 47.49 | 30.10 | 77.67 |
| Capture time (ns/tick) | 989.3 | 627.2 | 1618.2 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 7.88 | 74.33 | 0.38 |
| Write CPU time (ms) | 55.47 | 84.24 | 64.14 |
| Write min / max (ms) | 55.50 / 57.50 | 102.97 / 106.15 | 78.01 / 78.81 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.523 | 0.340 | 19.170 |
| search-signal: p95 (ms) | 0.978 | 0.476 | 46.285 |
| search-signal: peak RSS (MiB) | 7.04 | 15.07 | 6.92 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.614 | 0.315 | 48.407 |
| find-signal: p95 (ms) | 0.951 | 0.479 | 52.650 |
| find-signal: peak RSS (MiB) | 7.63 | 15.86 | 6.66 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.506 | 0.274 | 18.763 |
| point: p95 (ms) | 0.899 | 0.410 | 46.396 |
| point: peak RSS (MiB) | 7.09 | 5.95 | 6.70 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.87 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 33.90 | 39.47 |
| Write throughput (Mobs/s) | 6.068 | 5.213 |
| Read time (ms) | 16.04 | 13.01 |
| Read throughput (Mobs/s) | 12.829 | 15.812 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 5.94 | 5.70 |
| Read peak RSS (MiB) | 5.01 | 7.57 |
| Capture time (ms) | 33.08 | 36.35 |
| Capture time (ns/tick) | 275.7 | 302.9 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.36 | 2.95 |
| Write CPU time (ms) | 29.70 | 34.12 |
| Write min / max (ms) | 33.77 / 34.59 | 38.85 / 40.33 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.593 | 5.079 |
| search-transaction: p95 (ms) | 0.865 | 6.110 |
| search-transaction: peak RSS (MiB) | 5.01 | 7.57 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 4.911 | 6.507 |
| find-transaction: p95 (ms) | 5.240 | 6.730 |
| find-transaction: peak RSS (MiB) | 5.08 | 7.57 |

### many-generators

Observations: 329,168. Null producer: 16.00 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 58.44 | 67.85 |
| Write throughput (Mobs/s) | 5.632 | 4.852 |
| Read time (ms) | 27.92 | 25.35 |
| Read throughput (Mobs/s) | 11.788 | 12.987 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 7.35 | 6.55 |
| Read peak RSS (MiB) | 5.84 | 10.40 |
| Capture time (ms) | 57.57 | 66.57 |
| Capture time (ns/tick) | 2398.7 | 2773.7 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.34 | 1.12 |
| Write CPU time (ms) | 51.68 | 62.48 |
| Write min / max (ms) | 58.18 / 59.45 | 67.35 / 75.07 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.977 | 3.173 |
| search-transaction: p95 (ms) | 1.273 | 4.022 |
| search-transaction: peak RSS (MiB) | 5.84 | 10.40 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 6.224 | 3.905 |
| find-transaction: p95 (ms) | 6.542 | 4.109 |
| find-transaction: peak RSS (MiB) | 5.85 | 10.40 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.91 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 35.98 | 63.31 | 56.54 |
| Write throughput (Mobs/s) | 37.278 | 21.186 | 23.721 |
| Read time (ms) | 16.37 | 34.28 | 50.58 |
| Read throughput (Mobs/s) | 81.945 | 39.124 | 26.517 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 17.07 | 41.91 | 6.84 |
| Read peak RSS (MiB) | 6.30 | 8.52 | 5.94 |
| Capture time (ms) | 31.50 | 30.82 | 55.66 |
| Capture time (ns/tick) | 1312.7 | 1284.0 | 2319.2 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 3.62 | 31.52 | 0.59 |
| Write CPU time (ms) | 33.31 | 57.82 | 48.13 |
| Write min / max (ms) | 35.31 / 37.99 | 62.08 / 65.62 | 56.51 / 57.29 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.411 | 0.240 | 18.790 |
| search-signal: p95 (ms) | 0.777 | 0.522 | 26.020 |
| search-signal: peak RSS (MiB) | 4.99 | 10.12 | 7.24 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.429 | 0.257 | 28.166 |
| find-signal: p95 (ms) | 0.757 | 0.505 | 28.678 |
| find-signal: peak RSS (MiB) | 6.82 | 10.64 | 6.84 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.409 | 0.150 | 18.903 |
| point: p95 (ms) | 0.678 | 0.221 | 26.486 |
| point: peak RSS (MiB) | 6.55 | 5.74 | 7.34 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.338 | 0.949 | 0.970 |
| search-transaction: p95 (ms) | 0.559 | 1.595 | 1.621 |
| search-transaction: peak RSS (MiB) | 4.70 | 4.94 | 4.94 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.934 | 1.621 | 1.621 |
| find-transaction: p95 (ms) | 2.220 | 1.705 | 1.718 |
| find-transaction: peak RSS (MiB) | 6.58 | 6.92 | 4.94 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
