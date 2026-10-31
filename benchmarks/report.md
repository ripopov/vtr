# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **67.49 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 43.36 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 103.90 | 489.59 | 248.22 |
| Write throughput (Mobs/s) | 60.550 | 12.850 | 25.344 |
| Read time (ms) | 46.82 | 140.03 | 217.86 |
| Read throughput (Mobs/s) | 134.380 | 44.926 | 28.877 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 21.62 | 143.26 | 4.66 |
| Read peak RSS (MiB) | 12.85 | 25.67 | 4.64 |
| Capture time (ms) | 98.47 | 434.05 | 243.88 |
| Capture time (ns/tick) | 205.1 | 904.3 | 508.1 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 5.19 | 58.61 | 1.51 |
| Write CPU time (ms) | 98.44 | 476.85 | 157.17 |
| Write min / max (ms) | 103.88 / 113.85 | 463.75 / 575.94 | 208.03 / 253.81 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.566 | 1.382 | 78.214 |
| search-signal: p95 (ms) | 0.870 | 5.104 | 124.091 |
| search-signal: peak RSS (MiB) | 7.69 | 26.66 | 6.58 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 1.704 | 1.770 | 130.197 |
| find-signal: p95 (ms) | 4.089 | 6.798 | 142.150 |
| find-signal: peak RSS (MiB) | 7.78 | 27.45 | 6.34 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.583 | 0.533 | 80.581 |
| point: p95 (ms) | 0.893 | 1.175 | 127.461 |
| point: peak RSS (MiB) | 7.46 | 9.52 | 6.34 |

### soc

Observations: 20,143,793. Null producer: 116.21 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 466.58 | 1066.25 | 855.88 |
| Write throughput (Mobs/s) | 43.173 | 18.892 | 23.536 |
| Read time (ms) | 159.38 | 499.09 | 684.28 |
| Read throughput (Mobs/s) | 126.391 | 40.361 | 29.438 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 17.25 | 148.00 | 4.89 |
| Read peak RSS (MiB) | 31.13 | 23.57 | 4.29 |
| Capture time (ms) | 457.29 | 994.05 | 850.80 |
| Capture time (ns/tick) | 19053.7 | 41418.6 | 35449.9 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 9.65 | 68.09 | 2.14 |
| Write CPU time (ms) | 453.26 | 841.91 | 496.77 |
| Write min / max (ms) | 443.07 / 480.04 | 977.61 / 1081.97 | 766.72 / 917.53 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 1.022 | 0.608 | 259.896 |
| search-signal: p95 (ms) | 3.596 | 1.376 | 357.875 |
| search-signal: peak RSS (MiB) | 6.35 | 23.69 | 6.73 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 2.410 | 1.253 | 386.905 |
| find-signal: p95 (ms) | 3.257 | 1.899 | 432.427 |
| find-signal: peak RSS (MiB) | 10.49 | 26.70 | 6.90 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 1.011 | 0.434 | 256.323 |
| point: p95 (ms) | 1.623 | 0.560 | 371.226 |
| point: peak RSS (MiB) | 8.15 | 4.14 | 7.01 |

### idle

Observations: 2,547,072. Null producer: 14.55 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 58.31 | 105.64 | 103.84 |
| Write throughput (Mobs/s) | 43.679 | 24.111 | 24.529 |
| Read time (ms) | 19.41 | 59.42 | 82.77 |
| Read throughput (Mobs/s) | 131.202 | 42.867 | 30.774 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 16.28 | 79.00 | 4.90 |
| Read peak RSS (MiB) | 7.61 | 15.04 | 4.65 |
| Capture time (ms) | 48.59 | 29.93 | 102.01 |
| Capture time (ns/tick) | 1012.3 | 623.5 | 2125.3 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 8.43 | 74.41 | 1.50 |
| Write CPU time (ms) | 55.22 | 83.57 | 61.42 |
| Write min / max (ms) | 57.11 / 58.69 | 102.45 / 114.44 | 100.19 / 111.53 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.527 | 0.325 | 19.223 |
| search-signal: p95 (ms) | 0.821 | 0.403 | 47.487 |
| search-signal: peak RSS (MiB) | 7.45 | 16.03 | 7.18 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.613 | 0.318 | 51.310 |
| find-signal: p95 (ms) | 0.793 | 0.451 | 55.428 |
| find-signal: peak RSS (MiB) | 7.39 | 15.44 | 6.57 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.546 | 0.261 | 18.767 |
| point: p95 (ms) | 0.720 | 0.379 | 47.140 |
| point: peak RSS (MiB) | 7.54 | 6.12 | 7.25 |

## Transaction results

### requests

Observations: 205,728. Null producer: 8.23 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 35.82 | 38.97 |
| Write throughput (Mobs/s) | 5.743 | 5.279 |
| Read time (ms) | 15.58 | 13.28 |
| Read throughput (Mobs/s) | 13.206 | 15.491 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 7.23 | 4.12 |
| Read peak RSS (MiB) | 5.20 | 9.62 |
| Capture time (ms) | 32.57 | 36.14 |
| Capture time (ns/tick) | 271.4 | 301.1 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 2.82 | 2.84 |
| Write CPU time (ms) | 29.11 | 33.79 |
| Write min / max (ms) | 33.26 / 36.47 | 37.65 / 41.70 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.600 | 5.082 |
| search-transaction: p95 (ms) | 0.717 | 6.437 |
| search-transaction: peak RSS (MiB) | 5.20 | 7.69 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 4.435 | 6.451 |
| find-transaction: p95 (ms) | 4.639 | 6.662 |
| find-transaction: peak RSS (MiB) | 5.21 | 9.62 |

### many-generators

Observations: 329,168. Null producer: 15.82 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 58.32 | 73.72 |
| Write throughput (Mobs/s) | 5.644 | 4.465 |
| Read time (ms) | 26.98 | 25.46 |
| Read throughput (Mobs/s) | 12.202 | 12.926 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 5.77 | 6.90 |
| Read peak RSS (MiB) | 5.96 | 10.46 |
| Capture time (ms) | 56.98 | 65.91 |
| Capture time (ns/tick) | 2374.4 | 2746.1 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.37 | 7.64 |
| Write CPU time (ms) | 50.98 | 61.75 |
| Write min / max (ms) | 57.66 / 61.30 | 72.16 / 74.61 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.974 | 3.322 |
| search-transaction: p95 (ms) | 1.102 | 4.097 |
| search-transaction: peak RSS (MiB) | 5.96 | 10.46 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 5.461 | 4.010 |
| find-transaction: p95 (ms) | 5.888 | 4.217 |
| find-transaction: peak RSS (MiB) | 6.03 | 10.46 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.68 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 36.08 | 65.68 | 70.79 |
| Write throughput (Mobs/s) | 37.171 | 20.421 | 18.948 |
| Read time (ms) | 15.85 | 34.29 | 50.71 |
| Read throughput (Mobs/s) | 84.622 | 39.115 | 26.449 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.94 | 43.13 | 7.25 |
| Read peak RSS (MiB) | 8.48 | 10.20 | 6.00 |
| Capture time (ms) | 31.20 | 31.24 | 66.07 |
| Capture time (ns/tick) | 1300.0 | 1301.5 | 2753.1 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 3.67 | 34.16 | 3.33 |
| Write CPU time (ms) | 32.71 | 57.21 | 46.40 |
| Write min / max (ms) | 34.87 / 37.43 | 65.16 / 68.60 | 65.57 / 71.12 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.410 | 0.243 | 18.753 |
| search-signal: p95 (ms) | 0.559 | 0.517 | 27.107 |
| search-signal: peak RSS (MiB) | 5.11 | 10.80 | 7.39 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.429 | 0.245 | 28.234 |
| find-signal: p95 (ms) | 0.546 | 0.510 | 30.053 |
| find-signal: peak RSS (MiB) | 7.19 | 9.53 | 6.52 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.412 | 0.172 | 19.276 |
| point: p95 (ms) | 0.503 | 0.246 | 26.618 |
| point: peak RSS (MiB) | 6.85 | 4.18 | 6.76 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.333 | 0.994 | 0.974 |
| search-transaction: p95 (ms) | 0.410 | 1.628 | 1.599 |
| search-transaction: peak RSS (MiB) | 4.82 | 5.00 | 5.00 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.698 | 1.618 | 1.607 |
| find-transaction: p95 (ms) | 1.853 | 1.732 | 1.705 |
| find-transaction: peak RSS (MiB) | 4.82 | 5.00 | 5.00 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
