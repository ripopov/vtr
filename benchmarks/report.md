# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **52.76 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 47.67 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 109.42 | 654.58 | 159.74 |
| Write throughput (Mobs/s) | 57.492 | 9.611 | 39.384 |
| Read time (ms) | 38.65 | 165.71 | 181.82 |
| Read throughput (Mobs/s) | 162.762 | 37.965 | 34.600 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 18.98 | 141.05 | 5.58 |
| Read peak RSS (MiB) | 14.09 | 26.34 | 5.22 |
| Capture time (ms) | 107.79 | 557.21 | 159.13 |
| Capture time (ns/tick) | 224.6 | 1160.8 | 331.5 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.47 | 97.25 | 0.12 |
| Write CPU time (ms) | 110.26 | 655.27 | 160.43 |
| Write min / max (ms) | 109.18 / 110.46 | 642.46 / 729.57 | 159.40 / 160.89 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.112 | 2.424 | 57.361 |
| search-signal: p95 (ms) | 0.383 | 6.229 | 90.821 |
| search-signal: peak RSS (MiB) | 7.76 | 26.99 | 6.69 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.619 | 2.959 | 93.340 |
| find-signal: p95 (ms) | 2.695 | 8.340 | 95.820 |
| find-signal: peak RSS (MiB) | 10.26 | 27.01 | 6.67 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.118 | 0.352 | 55.441 |
| point: p95 (ms) | 0.453 | 1.383 | 87.512 |
| point: peak RSS (MiB) | 7.72 | 8.99 | 6.68 |

### soc

Observations: 20,143,793. Null producer: 138.24 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 402.68 | 813.67 | 489.82 |
| Write throughput (Mobs/s) | 50.025 | 24.757 | 41.125 |
| Read time (ms) | 117.89 | 579.10 | 561.74 |
| Read throughput (Mobs/s) | 170.868 | 34.785 | 35.860 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 16.22 | 145.56 | 5.71 |
| Read peak RSS (MiB) | 30.77 | 22.68 | 5.27 |
| Capture time (ms) | 395.08 | 789.66 | 488.97 |
| Capture time (ns/tick) | 16461.6 | 32902.4 | 20373.8 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 6.32 | 23.39 | 0.14 |
| Write CPU time (ms) | 403.43 | 813.87 | 490.40 |
| Write min / max (ms) | 398.65 / 408.63 | 812.59 / 814.75 | 485.15 / 501.93 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.414 | 0.427 | 182.381 |
| search-signal: p95 (ms) | 0.619 | 6.302 | 254.478 |
| search-signal: peak RSS (MiB) | 7.15 | 22.70 | 6.68 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.553 | 7.036 | 277.240 |
| find-signal: p95 (ms) | 0.769 | 7.383 | 282.447 |
| find-signal: peak RSS (MiB) | 9.47 | 22.32 | 6.71 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.406 | 0.196 | 178.658 |
| point: p95 (ms) | 0.597 | 0.290 | 254.489 |
| point: peak RSS (MiB) | 7.07 | 5.09 | 6.67 |

### idle

Observations: 2,547,072. Null producer: 17.68 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 56.28 | 112.30 | 63.63 |
| Write throughput (Mobs/s) | 45.259 | 22.681 | 40.028 |
| Read time (ms) | 16.89 | 71.96 | 72.46 |
| Read throughput (Mobs/s) | 150.814 | 35.393 | 35.152 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.44 | 77.67 | 5.61 |
| Read peak RSS (MiB) | 7.82 | 14.60 | 5.24 |
| Capture time (ms) | 47.45 | 57.86 | 62.77 |
| Capture time (ns/tick) | 988.4 | 1205.3 | 1307.7 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 7.60 | 54.02 | 0.15 |
| Write CPU time (ms) | 57.11 | 113.00 | 64.28 |
| Write min / max (ms) | 56.26 / 57.40 | 111.56 / 112.88 | 63.10 / 63.82 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.408 | 0.322 | 13.830 |
| search-signal: p95 (ms) | 0.654 | 3.898 | 34.205 |
| search-signal: peak RSS (MiB) | 6.83 | 15.12 | 6.64 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.417 | 0.326 | 35.793 |
| find-signal: p95 (ms) | 0.705 | 3.966 | 36.842 |
| find-signal: peak RSS (MiB) | 6.83 | 15.09 | 6.70 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.394 | 0.092 | 13.685 |
| point: p95 (ms) | 0.618 | 0.280 | 34.860 |
| point: peak RSS (MiB) | 6.88 | 5.51 | 6.66 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.12 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 27.97 | 36.45 |
| Write throughput (Mobs/s) | 7.356 | 5.644 |
| Read time (ms) | 21.04 | 11.29 |
| Read throughput (Mobs/s) | 9.780 | 18.226 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.39 | 4.98 |
| Read peak RSS (MiB) | 6.30 | 8.79 |
| Capture time (ms) | 27.66 | 36.24 |
| Capture time (ns/tick) | 230.5 | 302.0 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.12 | 0.16 |
| Write CPU time (ms) | 28.91 | 37.16 |
| Write min / max (ms) | 27.83 / 28.70 | 36.31 / 36.59 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.112 | 3.386 |
| search-transaction: p95 (ms) | 0.198 | 4.105 |
| search-transaction: peak RSS (MiB) | 6.40 | 8.81 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 8.632 | 4.843 |
| find-transaction: p95 (ms) | 10.410 | 4.980 |
| find-transaction: peak RSS (MiB) | 6.45 | 8.79 |

### many-generators

Observations: 329,168. Null producer: 13.84 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 50.36 | 65.59 |
| Write throughput (Mobs/s) | 6.536 | 5.018 |
| Read time (ms) | 33.70 | 20.40 |
| Read throughput (Mobs/s) | 9.768 | 16.133 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.50 | 6.18 |
| Read peak RSS (MiB) | 7.06 | 11.85 |
| Capture time (ms) | 50.03 | 64.87 |
| Capture time (ns/tick) | 2084.5 | 2702.9 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.14 | 0.67 |
| Write CPU time (ms) | 51.12 | 66.41 |
| Write min / max (ms) | 50.25 / 50.80 | 65.12 / 66.03 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.288 | 0.596 |
| search-transaction: p95 (ms) | 0.400 | 1.352 |
| search-transaction: peak RSS (MiB) | 7.21 | 11.99 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 11.978 | 1.286 |
| find-transaction: p95 (ms) | 13.827 | 1.372 |
| find-transaction: peak RSS (MiB) | 7.24 | 11.98 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.77 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 36.88 | 87.07 | 47.86 |
| Write throughput (Mobs/s) | 36.367 | 15.405 | 28.024 |
| Read time (ms) | 15.92 | 37.77 | 40.71 |
| Read throughput (Mobs/s) | 84.250 | 35.515 | 32.950 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.46 | 41.06 | 6.10 |
| Read peak RSS (MiB) | 7.50 | 9.41 | 6.18 |
| Capture time (ms) | 32.32 | 45.84 | 47.16 |
| Capture time (ns/tick) | 1346.7 | 1910.0 | 1964.9 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 4.25 | 41.95 | 0.25 |
| Write CPU time (ms) | 37.67 | 87.79 | 48.53 |
| Write min / max (ms) | 36.13 / 36.91 | 86.39 / 88.26 | 47.55 / 48.44 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.080 | 0.220 | 12.124 |
| search-signal: p95 (ms) | 0.256 | 1.900 | 16.462 |
| search-signal: peak RSS (MiB) | 6.87 | 9.87 | 6.69 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.095 | 0.211 | 17.904 |
| find-signal: p95 (ms) | 0.253 | 1.774 | 18.728 |
| find-signal: peak RSS (MiB) | 6.67 | 9.87 | 6.66 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.084 | 0.034 | 11.670 |
| point: p95 (ms) | 0.249 | 0.139 | 16.443 |
| point: peak RSS (MiB) | 6.80 | 5.23 | 6.64 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.140 | 0.442 | 0.426 |
| search-transaction: p95 (ms) | 0.209 | 1.069 | 1.087 |
| search-transaction: peak RSS (MiB) | 5.89 | 6.18 | 6.19 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 3.638 | 1.068 | 1.084 |
| find-transaction: p95 (ms) | 4.805 | 1.154 | 1.149 |
| find-transaction: peak RSS (MiB) | 5.89 | 6.14 | 6.13 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
