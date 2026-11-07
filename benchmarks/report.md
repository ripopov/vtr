# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **52.67 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-42-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 48.24 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 110.30 | 624.27 | 162.67 |
| Write throughput (Mobs/s) | 57.037 | 10.077 | 38.674 |
| Read time (ms) | 38.66 | 166.47 | 177.65 |
| Read throughput (Mobs/s) | 162.721 | 37.791 | 35.412 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 19.08 | 141.07 | 5.61 |
| Read peak RSS (MiB) | 14.07 | 26.42 | 5.39 |
| Capture time (ms) | 108.85 | 541.24 | 161.72 |
| Capture time (ns/tick) | 226.8 | 1127.6 | 336.9 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.28 | 78.37 | 0.55 |
| Write CPU time (ms) | 111.17 | 624.96 | 163.22 |
| Write min / max (ms) | 108.28 / 111.55 | 603.59 / 644.06 | 162.09 / 210.65 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.115 | 2.400 | 55.563 |
| search-signal: p95 (ms) | 0.390 | 6.139 | 87.801 |
| search-signal: peak RSS (MiB) | 7.72 | 27.09 | 6.75 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.630 | 2.835 | 91.725 |
| find-signal: p95 (ms) | 2.629 | 8.439 | 95.109 |
| find-signal: peak RSS (MiB) | 10.24 | 27.03 | 6.76 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.118 | 0.357 | 54.273 |
| point: p95 (ms) | 0.425 | 1.327 | 87.817 |
| point: peak RSS (MiB) | 7.83 | 8.97 | 6.80 |

### soc

Observations: 20,143,793. Null producer: 140.85 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 417.12 | 835.77 | 545.25 |
| Write throughput (Mobs/s) | 48.293 | 24.102 | 36.944 |
| Read time (ms) | 121.86 | 594.27 | 574.09 |
| Read throughput (Mobs/s) | 165.304 | 33.896 | 35.088 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 16.32 | 145.61 | 5.77 |
| Read peak RSS (MiB) | 30.71 | 22.75 | 5.38 |
| Capture time (ms) | 409.09 | 810.99 | 544.28 |
| Capture time (ns/tick) | 17045.3 | 33791.0 | 22678.4 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 6.69 | 24.19 | 0.16 |
| Write CPU time (ms) | 418.09 | 836.44 | 546.19 |
| Write min / max (ms) | 405.33 / 433.57 | 824.52 / 840.96 | 513.97 / 615.55 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.421 | 0.448 | 184.291 |
| search-signal: p95 (ms) | 0.674 | 6.911 | 265.464 |
| search-signal: peak RSS (MiB) | 7.20 | 22.82 | 6.71 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.584 | 7.463 | 277.875 |
| find-signal: p95 (ms) | 0.796 | 8.117 | 280.339 |
| find-signal: peak RSS (MiB) | 9.45 | 22.47 | 6.80 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.422 | 0.203 | 181.910 |
| point: p95 (ms) | 0.644 | 0.317 | 258.111 |
| point: peak RSS (MiB) | 7.11 | 5.09 | 6.75 |

### idle

Observations: 2,547,072. Null producer: 17.84 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 57.34 | 113.88 | 66.43 |
| Write throughput (Mobs/s) | 44.420 | 22.365 | 38.344 |
| Read time (ms) | 16.97 | 73.78 | 71.34 |
| Read throughput (Mobs/s) | 150.134 | 34.520 | 35.703 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.44 | 77.69 | 5.76 |
| Read peak RSS (MiB) | 7.94 | 14.78 | 5.42 |
| Capture time (ms) | 48.68 | 58.04 | 65.60 |
| Capture time (ns/tick) | 1014.2 | 1209.2 | 1366.7 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 7.38 | 55.25 | 0.17 |
| Write CPU time (ms) | 58.03 | 114.66 | 67.06 |
| Write min / max (ms) | 56.71 / 58.69 | 112.06 / 115.71 | 66.16 / 66.67 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.389 | 0.328 | 13.609 |
| search-signal: p95 (ms) | 0.678 | 4.132 | 33.908 |
| search-signal: peak RSS (MiB) | 6.77 | 15.18 | 6.74 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.414 | 0.333 | 34.198 |
| find-signal: p95 (ms) | 0.701 | 4.055 | 34.906 |
| find-signal: peak RSS (MiB) | 6.77 | 15.12 | 6.71 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.410 | 0.095 | 13.303 |
| point: p95 (ms) | 0.623 | 0.297 | 33.361 |
| point: peak RSS (MiB) | 6.82 | 5.54 | 6.76 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.23 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 28.49 | 37.11 |
| Write throughput (Mobs/s) | 7.222 | 5.544 |
| Read time (ms) | 14.91 | 11.20 |
| Read throughput (Mobs/s) | 13.796 | 18.361 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.41 | 5.06 |
| Read peak RSS (MiB) | 6.25 | 8.82 |
| Capture time (ms) | 28.18 | 36.90 |
| Capture time (ns/tick) | 234.8 | 307.5 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.12 | 0.16 |
| Write CPU time (ms) | 29.39 | 37.74 |
| Write min / max (ms) | 28.48 / 29.28 | 36.64 / 37.83 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.074 | 3.381 |
| search-transaction: p95 (ms) | 0.145 | 4.267 |
| search-transaction: peak RSS (MiB) | 6.18 | 8.92 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 4.245 | 4.853 |
| find-transaction: p95 (ms) | 4.351 | 5.041 |
| find-transaction: peak RSS (MiB) | 6.17 | 8.76 |

### many-generators

Observations: 329,168. Null producer: 13.83 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 51.48 | 66.26 |
| Write throughput (Mobs/s) | 6.394 | 4.968 |
| Read time (ms) | 24.53 | 20.57 |
| Read throughput (Mobs/s) | 13.421 | 16.002 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.59 | 6.25 |
| Read peak RSS (MiB) | 7.05 | 12.02 |
| Capture time (ms) | 51.16 | 65.54 |
| Capture time (ns/tick) | 2131.8 | 2730.8 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.14 | 0.69 |
| Write CPU time (ms) | 52.27 | 67.12 |
| Write min / max (ms) | 51.43 / 52.41 | 66.26 / 68.26 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.159 | 0.609 |
| search-transaction: p95 (ms) | 0.273 | 1.315 |
| search-transaction: peak RSS (MiB) | 7.09 | 12.00 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 4.842 | 1.286 |
| find-transaction: p95 (ms) | 5.137 | 1.361 |
| find-transaction: peak RSS (MiB) | 7.06 | 12.04 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.93 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 37.43 | 87.27 | 49.94 |
| Write throughput (Mobs/s) | 35.835 | 15.369 | 26.856 |
| Read time (ms) | 13.94 | 38.67 | 40.99 |
| Read throughput (Mobs/s) | 96.223 | 34.683 | 32.719 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.72 | 41.11 | 6.27 |
| Read peak RSS (MiB) | 7.56 | 9.52 | 6.27 |
| Capture time (ms) | 32.83 | 45.99 | 49.18 |
| Capture time (ns/tick) | 1367.8 | 1916.4 | 2049.1 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 4.38 | 40.29 | 0.26 |
| Write CPU time (ms) | 38.22 | 87.94 | 50.71 |
| Write min / max (ms) | 37.34 / 37.69 | 84.24 / 89.94 | 49.75 / 50.46 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.082 | 0.234 | 11.829 |
| search-signal: p95 (ms) | 0.246 | 2.015 | 16.863 |
| search-signal: peak RSS (MiB) | 6.69 | 9.92 | 6.66 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.092 | 0.215 | 17.848 |
| find-signal: p95 (ms) | 0.239 | 1.808 | 18.470 |
| find-signal: peak RSS (MiB) | 6.70 | 9.98 | 6.74 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.086 | 0.036 | 11.900 |
| point: p95 (ms) | 0.224 | 0.144 | 16.362 |
| point: peak RSS (MiB) | 6.68 | 5.29 | 6.76 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.081 | 0.438 | 0.432 |
| search-transaction: p95 (ms) | 0.153 | 1.119 | 1.088 |
| search-transaction: peak RSS (MiB) | 5.96 | 6.21 | 6.21 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.658 | 1.075 | 1.072 |
| find-transaction: p95 (ms) | 1.754 | 1.164 | 1.159 |
| find-transaction: peak RSS (MiB) | 5.96 | 6.18 | 6.21 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
