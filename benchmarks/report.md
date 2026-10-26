# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **52.17 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 48.33 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 113.34 | 652.49 | 164.56 |
| Write throughput (Mobs/s) | 55.505 | 9.642 | 38.231 |
| Read time (ms) | 39.23 | 168.39 | 185.12 |
| Read throughput (Mobs/s) | 160.348 | 37.359 | 33.985 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 19.03 | 141.05 | 5.60 |
| Read peak RSS (MiB) | 14.24 | 26.24 | 5.23 |
| Capture time (ms) | 111.86 | 566.76 | 163.94 |
| Capture time (ns/tick) | 233.0 | 1180.7 | 341.5 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.47 | 92.58 | 0.09 |
| Write CPU time (ms) | 114.13 | 653.35 | 165.34 |
| Write min / max (ms) | 111.50 / 113.53 | 651.26 / 769.14 | 158.10 / 165.53 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.116 | 2.446 | 55.703 |
| search-signal: p95 (ms) | 0.405 | 6.323 | 89.818 |
| search-signal: peak RSS (MiB) | 7.88 | 26.98 | 6.68 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.644 | 2.957 | 95.998 |
| find-signal: p95 (ms) | 2.721 | 8.656 | 134.624 |
| find-signal: peak RSS (MiB) | 10.33 | 27.00 | 6.61 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.119 | 0.391 | 58.095 |
| point: p95 (ms) | 0.454 | 1.406 | 90.589 |
| point: peak RSS (MiB) | 7.75 | 8.97 | 6.73 |

### soc

Observations: 20,143,793. Null producer: 136.98 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 383.71 | 802.29 | 477.44 |
| Write throughput (Mobs/s) | 52.498 | 25.108 | 42.192 |
| Read time (ms) | 116.04 | 568.11 | 551.22 |
| Read throughput (Mobs/s) | 173.587 | 35.457 | 36.544 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 16.16 | 145.58 | 5.74 |
| Read peak RSS (MiB) | 30.75 | 22.64 | 5.13 |
| Capture time (ms) | 376.80 | 778.58 | 476.53 |
| Capture time (ns/tick) | 15700.0 | 32441.0 | 19855.5 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 5.76 | 23.17 | 0.14 |
| Write CPU time (ms) | 384.48 | 801.92 | 477.90 |
| Write min / max (ms) | 382.99 / 384.56 | 798.82 / 816.37 | 474.92 / 479.44 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.401 | 0.393 | 176.553 |
| search-signal: p95 (ms) | 0.578 | 5.757 | 249.252 |
| search-signal: peak RSS (MiB) | 7.00 | 22.70 | 6.64 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.528 | 6.498 | 270.224 |
| find-signal: p95 (ms) | 0.695 | 6.889 | 275.962 |
| find-signal: peak RSS (MiB) | 9.43 | 22.43 | 6.69 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.401 | 0.197 | 175.359 |
| point: p95 (ms) | 0.538 | 0.271 | 249.426 |
| point: peak RSS (MiB) | 7.07 | 5.11 | 6.64 |

### idle

Observations: 2,547,072. Null producer: 17.33 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 53.16 | 107.98 | 61.15 |
| Write throughput (Mobs/s) | 47.918 | 23.588 | 41.650 |
| Read time (ms) | 15.97 | 71.05 | 70.45 |
| Read throughput (Mobs/s) | 159.541 | 35.848 | 36.152 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.58 | 77.67 | 5.74 |
| Read peak RSS (MiB) | 7.95 | 14.66 | 5.23 |
| Capture time (ms) | 45.39 | 54.73 | 60.42 |
| Capture time (ns/tick) | 945.7 | 1140.2 | 1258.7 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 6.67 | 52.85 | 0.12 |
| Write CPU time (ms) | 53.73 | 108.49 | 61.68 |
| Write min / max (ms) | 52.91 / 54.78 | 107.91 / 108.59 | 60.88 / 61.24 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.378 | 0.329 | 13.391 |
| search-signal: p95 (ms) | 0.577 | 3.601 | 33.384 |
| search-signal: peak RSS (MiB) | 6.91 | 15.12 | 6.71 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.396 | 0.315 | 34.485 |
| find-signal: p95 (ms) | 0.628 | 3.324 | 35.289 |
| find-signal: peak RSS (MiB) | 6.73 | 15.09 | 6.69 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.392 | 0.093 | 13.136 |
| point: p95 (ms) | 0.569 | 0.255 | 33.229 |
| point: peak RSS (MiB) | 6.73 | 5.47 | 6.66 |

## Transaction results

### requests

Observations: 205,728. Null producer: 6.96 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 27.38 | 35.95 |
| Write throughput (Mobs/s) | 7.514 | 5.723 |
| Read time (ms) | 20.36 | 10.77 |
| Read throughput (Mobs/s) | 10.106 | 19.102 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.42 | 4.96 |
| Read peak RSS (MiB) | 6.34 | 8.84 |
| Capture time (ms) | 27.11 | 35.79 |
| Capture time (ns/tick) | 225.9 | 298.2 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.10 | 0.13 |
| Write CPU time (ms) | 28.07 | 36.50 |
| Write min / max (ms) | 27.11 / 28.11 | 35.44 / 36.20 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.107 | 3.222 |
| search-transaction: p95 (ms) | 0.156 | 4.129 |
| search-transaction: peak RSS (MiB) | 6.24 | 8.83 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 8.748 | 4.630 |
| find-transaction: p95 (ms) | 10.127 | 4.705 |
| find-transaction: peak RSS (MiB) | 6.27 | 8.84 |

### many-generators

Observations: 329,168. Null producer: 13.34 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 50.14 | 63.67 |
| Write throughput (Mobs/s) | 6.565 | 5.170 |
| Read time (ms) | 34.16 | 19.98 |
| Read throughput (Mobs/s) | 9.635 | 16.477 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.52 | 6.13 |
| Read peak RSS (MiB) | 7.08 | 12.03 |
| Capture time (ms) | 49.87 | 63.04 |
| Capture time (ns/tick) | 2077.9 | 2626.6 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.11 | 0.60 |
| Write CPU time (ms) | 50.70 | 64.23 |
| Write min / max (ms) | 49.48 / 50.66 | 63.43 / 64.59 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.279 | 0.575 |
| search-transaction: p95 (ms) | 0.374 | 1.286 |
| search-transaction: peak RSS (MiB) | 7.17 | 12.00 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 11.808 | 1.235 |
| find-transaction: p95 (ms) | 13.930 | 1.312 |
| find-transaction: peak RSS (MiB) | 7.09 | 11.96 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.50 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 34.91 | 81.04 | 46.12 |
| Write throughput (Mobs/s) | 38.416 | 16.552 | 29.085 |
| Read time (ms) | 15.42 | 36.68 | 39.59 |
| Read throughput (Mobs/s) | 86.966 | 36.569 | 33.879 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.71 | 41.09 | 6.14 |
| Read peak RSS (MiB) | 7.45 | 9.46 | 6.13 |
| Capture time (ms) | 31.02 | 43.00 | 45.51 |
| Capture time (ns/tick) | 1292.6 | 1791.6 | 1896.2 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 3.69 | 37.89 | 0.20 |
| Write CPU time (ms) | 35.47 | 81.55 | 46.63 |
| Write min / max (ms) | 34.63 / 34.94 | 80.86 / 81.77 | 45.87 / 46.48 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.080 | 0.193 | 11.276 |
| search-signal: p95 (ms) | 0.202 | 1.650 | 15.885 |
| search-signal: peak RSS (MiB) | 6.71 | 9.84 | 6.71 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.094 | 0.187 | 17.040 |
| find-signal: p95 (ms) | 0.233 | 1.566 | 17.829 |
| find-signal: peak RSS (MiB) | 6.69 | 9.81 | 6.68 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.083 | 0.034 | 11.058 |
| point: p95 (ms) | 0.206 | 0.123 | 15.906 |
| point: peak RSS (MiB) | 6.78 | 5.20 | 6.65 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.142 | 0.419 | 0.410 |
| search-transaction: p95 (ms) | 0.200 | 1.055 | 1.011 |
| search-transaction: peak RSS (MiB) | 5.77 | 6.18 | 6.15 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 3.676 | 1.044 | 1.028 |
| find-transaction: p95 (ms) | 4.547 | 1.080 | 1.087 |
| find-transaction: peak RSS (MiB) | 5.93 | 6.17 | 6.21 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
