# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **52.06 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 47.97 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 110.41 | 568.49 | 162.72 |
| Write throughput (Mobs/s) | 56.978 | 11.066 | 38.662 |
| Read time (ms) | 38.44 | 167.46 | 180.99 |
| Read throughput (Mobs/s) | 163.651 | 37.567 | 34.759 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 18.77 | 140.96 | 5.51 |
| Read peak RSS (MiB) | 14.00 | 26.22 | 5.21 |
| Capture time (ms) | 108.93 | 498.08 | 162.12 |
| Capture time (ns/tick) | 226.9 | 1037.7 | 337.7 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.33 | 70.29 | 0.11 |
| Write CPU time (ms) | 111.12 | 569.31 | 163.35 |
| Write min / max (ms) | 107.62 / 113.47 | 555.05 / 658.26 | 161.66 / 164.62 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.112 | 2.389 | 55.565 |
| search-signal: p95 (ms) | 0.377 | 6.097 | 90.195 |
| search-signal: peak RSS (MiB) | 7.60 | 26.91 | 6.64 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.620 | 2.827 | 91.871 |
| find-signal: p95 (ms) | 2.683 | 8.481 | 94.950 |
| find-signal: peak RSS (MiB) | 10.12 | 26.93 | 6.66 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.122 | 0.357 | 55.108 |
| point: p95 (ms) | 0.433 | 1.235 | 88.461 |
| point: peak RSS (MiB) | 7.58 | 8.89 | 6.63 |

### soc

Observations: 20,143,793. Null producer: 139.54 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 397.85 | 826.32 | 506.26 |
| Write throughput (Mobs/s) | 50.632 | 24.378 | 39.789 |
| Read time (ms) | 119.94 | 587.71 | 567.65 |
| Read throughput (Mobs/s) | 167.946 | 34.275 | 35.486 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 16.18 | 145.45 | 5.59 |
| Read peak RSS (MiB) | 30.52 | 22.59 | 5.29 |
| Capture time (ms) | 390.63 | 801.44 | 505.39 |
| Capture time (ns/tick) | 16276.2 | 33393.3 | 21057.8 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 5.94 | 23.66 | 0.15 |
| Write CPU time (ms) | 398.56 | 826.97 | 506.84 |
| Write min / max (ms) | 396.07 / 401.11 | 824.72 / 829.92 | 505.64 / 511.40 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.413 | 0.411 | 180.997 |
| search-signal: p95 (ms) | 0.627 | 6.299 | 257.525 |
| search-signal: peak RSS (MiB) | 6.94 | 22.62 | 6.68 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.556 | 7.027 | 275.394 |
| find-signal: p95 (ms) | 0.737 | 7.519 | 278.989 |
| find-signal: peak RSS (MiB) | 9.32 | 22.35 | 6.58 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.416 | 0.192 | 179.469 |
| point: p95 (ms) | 0.563 | 0.284 | 254.490 |
| point: peak RSS (MiB) | 6.95 | 4.97 | 6.59 |

### idle

Observations: 2,547,072. Null producer: 17.27 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 56.16 | 114.56 | 64.62 |
| Write throughput (Mobs/s) | 45.352 | 22.233 | 39.418 |
| Read time (ms) | 17.10 | 72.96 | 72.56 |
| Read throughput (Mobs/s) | 148.927 | 34.912 | 35.101 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.29 | 77.54 | 5.57 |
| Read peak RSS (MiB) | 7.70 | 14.64 | 5.22 |
| Capture time (ms) | 47.78 | 59.00 | 63.81 |
| Capture time (ns/tick) | 995.4 | 1229.2 | 1329.3 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 7.04 | 55.18 | 0.16 |
| Write CPU time (ms) | 56.78 | 115.19 | 65.25 |
| Write min / max (ms) | 55.80 / 56.52 | 114.21 / 115.13 | 63.37 / 65.45 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.395 | 0.322 | 13.431 |
| search-signal: p95 (ms) | 0.609 | 3.793 | 32.882 |
| search-signal: peak RSS (MiB) | 6.66 | 15.04 | 6.60 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.408 | 0.320 | 34.711 |
| find-signal: p95 (ms) | 0.644 | 3.487 | 35.563 |
| find-signal: peak RSS (MiB) | 6.60 | 15.07 | 6.62 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.402 | 0.094 | 13.339 |
| point: p95 (ms) | 0.603 | 0.277 | 33.937 |
| point: peak RSS (MiB) | 6.63 | 5.41 | 6.61 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.18 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 28.12 | 37.01 |
| Write throughput (Mobs/s) | 7.316 | 5.558 |
| Read time (ms) | 15.22 | 11.15 |
| Read throughput (Mobs/s) | 13.514 | 18.458 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.34 | 4.91 |
| Read peak RSS (MiB) | 6.15 | 8.79 |
| Capture time (ms) | 27.85 | 36.83 |
| Capture time (ns/tick) | 232.1 | 306.9 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.12 | 0.15 |
| Write CPU time (ms) | 28.91 | 37.69 |
| Write min / max (ms) | 27.88 / 28.98 | 36.61 / 37.46 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.076 | 3.375 |
| search-transaction: p95 (ms) | 0.152 | 4.240 |
| search-transaction: peak RSS (MiB) | 6.21 | 8.79 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 4.696 | 4.925 |
| find-transaction: p95 (ms) | 4.943 | 5.123 |
| find-transaction: peak RSS (MiB) | 6.27 | 8.79 |

### many-generators

Observations: 329,168. Null producer: 13.78 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 50.68 | 65.88 |
| Write throughput (Mobs/s) | 6.496 | 4.996 |
| Read time (ms) | 25.02 | 20.86 |
| Read throughput (Mobs/s) | 13.157 | 15.779 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.31 | 6.09 |
| Read peak RSS (MiB) | 6.93 | 11.83 |
| Capture time (ms) | 50.34 | 65.14 |
| Capture time (ns/tick) | 2097.4 | 2714.3 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.13 | 0.66 |
| Write CPU time (ms) | 51.41 | 66.51 |
| Write min / max (ms) | 50.48 / 51.70 | 64.75 / 68.13 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.165 | 0.616 |
| search-transaction: p95 (ms) | 0.258 | 1.313 |
| search-transaction: peak RSS (MiB) | 6.96 | 11.86 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 5.509 | 1.302 |
| find-transaction: p95 (ms) | 5.878 | 1.471 |
| find-transaction: peak RSS (MiB) | 7.03 | 11.87 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.95 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 37.16 | 83.14 | 49.14 |
| Write throughput (Mobs/s) | 36.098 | 16.133 | 27.295 |
| Read time (ms) | 13.93 | 38.12 | 40.98 |
| Read throughput (Mobs/s) | 96.296 | 35.183 | 32.729 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.66 | 41.07 | 6.10 |
| Read peak RSS (MiB) | 7.41 | 9.39 | 6.15 |
| Capture time (ms) | 32.77 | 45.05 | 48.43 |
| Capture time (ns/tick) | 1365.5 | 1877.0 | 2017.8 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 4.08 | 37.41 | 0.25 |
| Write CPU time (ms) | 37.77 | 83.75 | 49.73 |
| Write min / max (ms) | 36.95 / 37.22 | 81.82 / 83.85 | 48.88 / 49.52 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.081 | 0.217 | 11.913 |
| search-signal: p95 (ms) | 0.214 | 1.866 | 16.779 |
| search-signal: peak RSS (MiB) | 6.54 | 9.76 | 6.62 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.092 | 0.201 | 17.852 |
| find-signal: p95 (ms) | 0.237 | 1.647 | 18.774 |
| find-signal: peak RSS (MiB) | 6.69 | 9.85 | 6.66 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.085 | 0.037 | 11.719 |
| point: p95 (ms) | 0.218 | 0.141 | 17.431 |
| point: peak RSS (MiB) | 6.59 | 5.14 | 6.63 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.083 | 0.426 | 0.457 |
| search-transaction: p95 (ms) | 0.151 | 1.106 | 3.472 |
| search-transaction: peak RSS (MiB) | 5.69 | 6.10 | 6.02 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.774 | 1.079 | 1.088 |
| find-transaction: p95 (ms) | 1.886 | 1.211 | 4.115 |
| find-transaction: peak RSS (MiB) | 5.75 | 6.05 | 6.09 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
