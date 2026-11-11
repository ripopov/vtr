# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **51.59 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 45.84 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 102.16 | 477.68 | 150.58 |
| Write throughput (Mobs/s) | 61.578 | 13.170 | 41.780 |
| Read time (ms) | 43.53 | 138.34 | 169.23 |
| Read throughput (Mobs/s) | 144.517 | 45.474 | 37.174 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 21.37 | 143.14 | 5.66 |
| Read peak RSS (MiB) | 14.29 | 25.87 | 4.23 |
| Capture time (ms) | 100.81 | 417.53 | 150.27 |
| Capture time (ns/tick) | 210.0 | 869.9 | 313.1 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.33 | 60.22 | 0.06 |
| Write CPU time (ms) | 102.67 | 478.16 | 151.04 |
| Write min / max (ms) | 99.85 / 103.09 | 476.79 / 527.20 | 144.47 / 157.49 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.251 | 1.172 | 59.942 |
| search-signal: p95 (ms) | 0.481 | 5.075 | 102.018 |
| search-signal: peak RSS (MiB) | 7.98 | 27.82 | 7.26 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 1.353 | 1.584 | 99.334 |
| find-signal: p95 (ms) | 3.854 | 6.309 | 110.622 |
| find-signal: peak RSS (MiB) | 11.00 | 27.69 | 7.23 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.263 | 0.374 | 64.875 |
| point: p95 (ms) | 0.575 | 1.176 | 116.859 |
| point: peak RSS (MiB) | 7.99 | 9.54 | 7.34 |

### soc

Observations: 20,143,793. Null producer: 118.58 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 385.42 | 673.30 | 439.64 |
| Write throughput (Mobs/s) | 52.264 | 29.918 | 45.819 |
| Read time (ms) | 134.27 | 453.17 | 569.67 |
| Read throughput (Mobs/s) | 150.027 | 44.451 | 35.360 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 17.50 | 147.18 | 4.90 |
| Read peak RSS (MiB) | 31.61 | 23.18 | 5.70 |
| Capture time (ms) | 378.62 | 654.73 | 438.92 |
| Capture time (ns/tick) | 15775.6 | 27280.3 | 18288.2 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 6.14 | 18.25 | 0.13 |
| Write CPU time (ms) | 386.09 | 673.77 | 440.15 |
| Write min / max (ms) | 379.00 / 390.94 | 667.34 / 718.72 | 417.11 / 710.33 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.300 | 0.272 | 188.067 |
| search-signal: p95 (ms) | 0.480 | 0.578 | 345.912 |
| search-signal: peak RSS (MiB) | 8.67 | 23.02 | 6.57 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.433 | 0.577 | 272.823 |
| find-signal: p95 (ms) | 0.614 | 1.523 | 365.367 |
| find-signal: peak RSS (MiB) | 12.25 | 26.35 | 7.34 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.294 | 0.149 | 178.446 |
| point: p95 (ms) | 0.470 | 0.283 | 255.494 |
| point: peak RSS (MiB) | 8.84 | 5.76 | 6.76 |

### idle

Observations: 2,547,072. Null producer: 15.51 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 53.23 | 73.41 | 55.27 |
| Write throughput (Mobs/s) | 47.846 | 34.695 | 46.086 |
| Read time (ms) | 17.82 | 59.10 | 66.15 |
| Read throughput (Mobs/s) | 142.922 | 43.098 | 38.505 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.97 | 78.04 | 4.90 |
| Read peak RSS (MiB) | 8.81 | 15.07 | 5.59 |
| Capture time (ms) | 45.54 | 29.24 | 54.73 |
| Capture time (ns/tick) | 948.8 | 609.2 | 1140.3 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 6.95 | 43.61 | 0.06 |
| Write CPU time (ms) | 53.69 | 73.84 | 55.66 |
| Write min / max (ms) | 52.13 / 55.10 | 73.31 / 74.94 | 54.02 / 56.43 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.315 | 0.154 | 14.332 |
| search-signal: p95 (ms) | 0.465 | 0.256 | 33.802 |
| search-signal: peak RSS (MiB) | 7.54 | 16.00 | 6.51 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.343 | 0.150 | 35.231 |
| find-signal: p95 (ms) | 0.548 | 0.256 | 38.876 |
| find-signal: peak RSS (MiB) | 7.02 | 15.42 | 6.64 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.309 | 0.103 | 13.448 |
| point: p95 (ms) | 0.495 | 0.204 | 33.799 |
| point: peak RSS (MiB) | 7.63 | 5.92 | 6.66 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.86 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 27.07 | 32.58 |
| Write throughput (Mobs/s) | 7.599 | 6.315 |
| Read time (ms) | 14.40 | 11.17 |
| Read throughput (Mobs/s) | 14.287 | 18.419 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.89 | 5.75 |
| Read peak RSS (MiB) | 5.25 | 9.64 |
| Capture time (ms) | 26.87 | 32.43 |
| Capture time (ns/tick) | 223.9 | 270.2 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.09 | 0.13 |
| Write CPU time (ms) | 27.51 | 32.98 |
| Write min / max (ms) | 26.96 / 27.54 | 32.48 / 32.96 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.088 | 3.631 |
| search-transaction: p95 (ms) | 0.127 | 4.562 |
| search-transaction: peak RSS (MiB) | 5.25 | 9.58 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 3.873 | 5.143 |
| find-transaction: p95 (ms) | 4.065 | 5.309 |
| find-transaction: peak RSS (MiB) | 5.25 | 9.61 |

### many-generators

Observations: 329,168. Null producer: 15.75 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 48.78 | 58.89 |
| Write throughput (Mobs/s) | 6.748 | 5.590 |
| Read time (ms) | 25.39 | 20.91 |
| Read throughput (Mobs/s) | 12.962 | 15.743 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 5.71 | 6.69 |
| Read peak RSS (MiB) | 6.00 | 10.87 |
| Capture time (ms) | 48.56 | 58.27 |
| Capture time (ns/tick) | 2023.3 | 2428.0 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.11 | 0.58 |
| Write CPU time (ms) | 49.22 | 59.32 |
| Write min / max (ms) | 48.75 / 48.79 | 58.64 / 69.52 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.189 | 0.679 |
| search-transaction: p95 (ms) | 0.244 | 1.456 |
| search-transaction: peak RSS (MiB) | 7.84 | 12.79 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 4.509 | 1.429 |
| find-transaction: p95 (ms) | 4.732 | 1.492 |
| find-transaction: peak RSS (MiB) | 8.04 | 10.81 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 12.14 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 31.82 | 53.98 | 42.15 |
| Write throughput (Mobs/s) | 42.155 | 24.850 | 31.824 |
| Read time (ms) | 14.46 | 31.43 | 41.28 |
| Read throughput (Mobs/s) | 92.737 | 42.670 | 32.493 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 16.39 | 43.20 | 6.95 |
| Read peak RSS (MiB) | 6.49 | 8.52 | 5.92 |
| Capture time (ms) | 29.18 | 29.24 | 41.72 |
| Capture time (ns/tick) | 1215.8 | 1218.2 | 1738.4 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 2.56 | 24.41 | 0.17 |
| Write CPU time (ms) | 32.28 | 54.38 | 42.59 |
| Write min / max (ms) | 31.50 / 32.17 | 52.86 / 57.93 | 42.07 / 42.39 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.111 | 0.110 | 14.201 |
| search-signal: p95 (ms) | 0.200 | 0.339 | 20.145 |
| search-signal: peak RSS (MiB) | 5.62 | 10.43 | 7.04 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.136 | 0.103 | 21.368 |
| find-signal: p95 (ms) | 0.257 | 0.336 | 21.915 |
| find-signal: peak RSS (MiB) | 7.54 | 9.82 | 6.95 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.117 | 0.038 | 14.027 |
| point: p95 (ms) | 0.198 | 0.094 | 20.142 |
| point: peak RSS (MiB) | 7.25 | 4.12 | 6.06 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.081 | 0.435 | 0.451 |
| search-transaction: p95 (ms) | 0.124 | 1.062 | 1.075 |
| search-transaction: peak RSS (MiB) | 4.84 | 6.89 | 5.04 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.390 | 1.060 | 1.052 |
| find-transaction: p95 (ms) | 1.538 | 1.111 | 1.121 |
| find-transaction: peak RSS (MiB) | 4.93 | 6.90 | 5.04 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
