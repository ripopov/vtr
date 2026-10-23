# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **51.65 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 47.72 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 108.08 | 557.47 | 158.57 |
| Write throughput (Mobs/s) | 58.208 | 11.285 | 39.675 |
| Read time (ms) | 37.56 | 166.13 | 176.30 |
| Read throughput (Mobs/s) | 167.487 | 37.869 | 35.684 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 19.11 | 140.96 | 5.43 |
| Read peak RSS (MiB) | 14.06 | 26.22 | 5.22 |
| Capture time (ms) | 106.65 | 486.93 | 158.07 |
| Capture time (ns/tick) | 222.2 | 1014.4 | 329.3 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.27 | 70.42 | 0.11 |
| Write CPU time (ms) | 108.64 | 558.18 | 159.07 |
| Write min / max (ms) | 106.22 / 109.86 | 546.42 / 604.96 | 157.77 / 159.66 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.109 | 2.337 | 54.779 |
| search-signal: p95 (ms) | 0.402 | 6.006 | 87.740 |
| search-signal: peak RSS (MiB) | 7.67 | 26.91 | 6.64 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.616 | 2.789 | 91.981 |
| find-signal: p95 (ms) | 2.637 | 8.216 | 95.486 |
| find-signal: peak RSS (MiB) | 10.26 | 26.94 | 6.62 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.117 | 0.349 | 55.065 |
| point: p95 (ms) | 0.431 | 1.211 | 89.248 |
| point: peak RSS (MiB) | 7.72 | 8.92 | 6.65 |

### soc

Observations: 20,143,793. Null producer: 136.84 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 391.23 | 806.21 | 486.40 |
| Write throughput (Mobs/s) | 51.488 | 24.986 | 41.414 |
| Read time (ms) | 116.80 | 575.06 | 549.58 |
| Read throughput (Mobs/s) | 172.458 | 35.029 | 36.653 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 16.03 | 145.49 | 5.60 |
| Read peak RSS (MiB) | 30.71 | 22.63 | 5.25 |
| Capture time (ms) | 384.09 | 782.82 | 485.30 |
| Capture time (ns/tick) | 16003.7 | 32617.7 | 20220.9 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 6.12 | 23.09 | 0.39 |
| Write CPU time (ms) | 391.90 | 806.65 | 486.55 |
| Write min / max (ms) | 390.84 / 403.17 | 801.17 / 809.12 | 477.95 / 498.15 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.412 | 0.394 | 177.766 |
| search-signal: p95 (ms) | 0.601 | 6.397 | 252.141 |
| search-signal: peak RSS (MiB) | 7.04 | 22.67 | 6.66 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.545 | 6.930 | 270.434 |
| find-signal: p95 (ms) | 0.727 | 7.805 | 275.378 |
| find-signal: peak RSS (MiB) | 9.46 | 22.38 | 6.62 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.411 | 0.190 | 178.284 |
| point: p95 (ms) | 0.565 | 0.282 | 251.476 |
| point: peak RSS (MiB) | 6.96 | 5.03 | 6.66 |

### idle

Observations: 2,547,072. Null producer: 17.20 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 53.75 | 109.57 | 61.57 |
| Write throughput (Mobs/s) | 47.391 | 23.245 | 41.366 |
| Read time (ms) | 16.12 | 71.36 | 69.77 |
| Read throughput (Mobs/s) | 157.961 | 35.691 | 36.507 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.15 | 77.49 | 5.62 |
| Read peak RSS (MiB) | 7.95 | 14.62 | 5.23 |
| Capture time (ms) | 45.84 | 55.99 | 60.71 |
| Capture time (ns/tick) | 954.9 | 1166.4 | 1264.8 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 6.72 | 53.08 | 0.14 |
| Write CPU time (ms) | 54.58 | 110.24 | 62.13 |
| Write min / max (ms) | 53.70 / 54.01 | 109.44 / 109.79 | 61.25 / 62.26 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.380 | 0.311 | 12.848 |
| search-signal: p95 (ms) | 0.608 | 3.743 | 32.014 |
| search-signal: peak RSS (MiB) | 6.74 | 15.09 | 6.66 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.397 | 0.315 | 33.223 |
| find-signal: p95 (ms) | 0.633 | 3.452 | 34.240 |
| find-signal: peak RSS (MiB) | 6.77 | 15.06 | 6.64 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.396 | 0.091 | 12.879 |
| point: p95 (ms) | 0.587 | 0.259 | 32.352 |
| point: peak RSS (MiB) | 6.78 | 5.48 | 6.67 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.07 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 28.58 | 36.89 |
| Write throughput (Mobs/s) | 7.198 | 5.576 |
| Read time (ms) | 21.76 | 10.83 |
| Read throughput (Mobs/s) | 9.454 | 19.002 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.38 | 5.00 |
| Read peak RSS (MiB) | 6.16 | 8.78 |
| Capture time (ms) | 28.30 | 36.71 |
| Capture time (ns/tick) | 235.8 | 305.9 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.11 | 0.14 |
| Write CPU time (ms) | 29.27 | 37.44 |
| Write min / max (ms) | 27.64 / 29.67 | 36.12 / 36.96 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.111 | 3.285 |
| search-transaction: p95 (ms) | 0.176 | 4.180 |
| search-transaction: peak RSS (MiB) | 6.18 | 8.74 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 8.690 | 4.653 |
| find-transaction: p95 (ms) | 9.566 | 4.813 |
| find-transaction: peak RSS (MiB) | 6.18 | 8.78 |

### many-generators

Observations: 329,168. Null producer: 14.07 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 52.61 | 65.17 |
| Write throughput (Mobs/s) | 6.257 | 5.051 |
| Read time (ms) | 37.08 | 20.32 |
| Read throughput (Mobs/s) | 8.877 | 16.201 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.41 | 6.18 |
| Read peak RSS (MiB) | 6.92 | 11.95 |
| Capture time (ms) | 52.28 | 64.50 |
| Capture time (ns/tick) | 2178.4 | 2687.4 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.13 | 0.63 |
| Write CPU time (ms) | 53.34 | 65.99 |
| Write min / max (ms) | 50.75 / 54.58 | 64.79 / 67.04 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.290 | 0.599 |
| search-transaction: p95 (ms) | 0.390 | 1.360 |
| search-transaction: peak RSS (MiB) | 7.04 | 11.83 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 12.019 | 1.276 |
| find-transaction: p95 (ms) | 14.571 | 1.347 |
| find-transaction: peak RSS (MiB) | 7.02 | 11.93 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.97 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 36.88 | 84.24 | 48.53 |
| Write throughput (Mobs/s) | 36.367 | 15.921 | 27.639 |
| Read time (ms) | 16.27 | 37.87 | 40.72 |
| Read throughput (Mobs/s) | 82.428 | 35.414 | 32.942 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.57 | 41.05 | 6.22 |
| Read peak RSS (MiB) | 7.42 | 9.38 | 6.12 |
| Capture time (ms) | 32.56 | 44.60 | 47.83 |
| Capture time (ns/tick) | 1356.5 | 1858.3 | 1992.9 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 4.09 | 37.49 | 0.23 |
| Write CPU time (ms) | 37.50 | 84.86 | 49.25 |
| Write min / max (ms) | 35.56 / 37.62 | 82.22 / 84.68 | 48.50 / 49.60 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.081 | 0.217 | 11.926 |
| search-signal: p95 (ms) | 0.232 | 1.925 | 16.788 |
| search-signal: peak RSS (MiB) | 6.63 | 9.88 | 6.61 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.098 | 0.203 | 17.777 |
| find-signal: p95 (ms) | 0.246 | 1.656 | 18.205 |
| find-signal: peak RSS (MiB) | 6.60 | 9.82 | 6.61 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.084 | 0.034 | 11.505 |
| point: p95 (ms) | 0.220 | 0.134 | 16.566 |
| point: peak RSS (MiB) | 6.61 | 5.18 | 6.67 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.144 | 0.426 | 0.425 |
| search-transaction: p95 (ms) | 0.201 | 1.084 | 1.043 |
| search-transaction: peak RSS (MiB) | 5.73 | 6.00 | 6.11 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 3.686 | 1.070 | 1.067 |
| find-transaction: p95 (ms) | 4.688 | 1.140 | 1.140 |
| find-transaction: peak RSS (MiB) | 5.88 | 6.10 | 6.14 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
