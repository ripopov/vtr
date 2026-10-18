# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **61.41 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 49.42 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 160.46 | 694.92 | 167.85 |
| Write throughput (Mobs/s) | 39.207 | 9.053 | 37.481 |
| Read time (ms) | 80.02 | 168.97 | 184.14 |
| Read throughput (Mobs/s) | 78.622 | 37.232 | 34.165 |
| File size (MiB) | 7.707 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.84 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 76.88 | 141.12 | 5.45 |
| Read peak RSS (MiB) | 40.82 | 26.25 | 5.21 |
| Capture time (ms) | 137.78 | 573.83 | 167.22 |
| Capture time (ns/tick) | 287.0 | 1195.5 | 348.4 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 22.45 | 120.93 | 0.12 |
| Write CPU time (ms) | 161.44 | 695.78 | 168.79 |
| Write min / max (ms) | 160.43 / 160.64 | 693.53 / 700.78 | 167.42 / 168.08 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.714 | 2.538 | 57.122 |
| search-signal: p95 (ms) | 2.314 | 6.271 | 90.871 |
| search-signal: peak RSS (MiB) | 12.82 | 26.95 | 6.66 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 2.660 | 3.118 | 96.200 |
| find-signal: p95 (ms) | 26.010 | 8.567 | 101.566 |
| find-signal: peak RSS (MiB) | 46.78 | 26.98 | 6.68 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.857 | 0.360 | 57.405 |
| point: p95 (ms) | 2.348 | 1.565 | 91.346 |
| point: peak RSS (MiB) | 12.81 | 8.91 | 6.69 |

### soc

Observations: 20,143,793. Null producer: 143.13 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 472.09 | 839.78 | 519.01 |
| Write throughput (Mobs/s) | 42.669 | 23.987 | 38.812 |
| Read time (ms) | 282.11 | 592.03 | 578.75 |
| Read throughput (Mobs/s) | 71.405 | 34.025 | 34.806 |
| File size (MiB) | 24.880 | 52.345 | 485.555 |
| Bytes per observation | 1.30 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.75 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 127.19 | 145.62 | 5.66 |
| Read peak RSS (MiB) | 81.75 | 22.65 | 5.23 |
| Capture time (ms) | 458.38 | 814.96 | 518.10 |
| Capture time (ns/tick) | 19099.3 | 33956.9 | 21587.6 |
| File bytes per tick | 1087.0 | 2287.0 | 21214.2 |
| Close time (ms) | 12.36 | 24.11 | 0.21 |
| Write CPU time (ms) | 473.11 | 840.63 | 519.99 |
| Write min / max (ms) | 471.34 / 472.31 | 838.53 / 840.71 | 518.18 / 520.41 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.320 | 0.435 | 183.111 |
| search-signal: p95 (ms) | 0.508 | 6.260 | 255.919 |
| search-signal: peak RSS (MiB) | 9.95 | 22.67 | 6.60 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.374 | 6.824 | 279.264 |
| find-signal: p95 (ms) | 1.423 | 7.106 | 286.417 |
| find-signal: peak RSS (MiB) | 12.79 | 22.29 | 6.60 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.330 | 0.199 | 186.017 |
| point: p95 (ms) | 0.489 | 0.320 | 264.806 |
| point: peak RSS (MiB) | 9.96 | 5.03 | 6.66 |

### idle

Observations: 2,547,072. Null producer: 17.97 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 74.30 | 115.71 | 67.43 |
| Write throughput (Mobs/s) | 34.283 | 22.013 | 37.776 |
| Read time (ms) | 35.55 | 74.01 | 73.54 |
| Read throughput (Mobs/s) | 71.651 | 34.415 | 34.634 |
| File size (MiB) | 1.831 | 4.165 | 61.551 |
| Bytes per observation | 0.75 | 1.71 | 25.34 |
| Logical bytes / file bytes | 20.19 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 45.72 | 77.48 | 5.59 |
| Read peak RSS (MiB) | 16.44 | 14.64 | 5.32 |
| Capture time (ms) | 60.61 | 59.00 | 66.49 |
| Capture time (ns/tick) | 1262.6 | 1229.2 | 1385.3 |
| File bytes per tick | 40.0 | 91.0 | 1344.6 |
| Close time (ms) | 12.69 | 56.09 | 0.18 |
| Write CPU time (ms) | 75.20 | 116.55 | 68.53 |
| Write min / max (ms) | 74.12 / 75.47 | 114.89 / 117.14 | 66.34 / 68.33 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.373 | 0.331 | 14.151 |
| search-signal: p95 (ms) | 0.580 | 4.253 | 34.819 |
| search-signal: peak RSS (MiB) | 7.13 | 15.07 | 6.67 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.357 | 0.345 | 36.563 |
| find-signal: p95 (ms) | 0.601 | 4.196 | 38.044 |
| find-signal: peak RSS (MiB) | 7.29 | 15.07 | 6.68 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.300 | 0.099 | 13.692 |
| point: p95 (ms) | 0.551 | 0.322 | 34.785 |
| point: peak RSS (MiB) | 7.14 | 5.50 | 6.68 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.29 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 51.81 | 38.59 |
| Write throughput (Mobs/s) | 3.971 | 5.332 |
| Read time (ms) | 56.55 | 11.38 |
| Read throughput (Mobs/s) | 3.638 | 18.084 |
| File size (MiB) | 2.312 | 4.424 |
| Bytes per observation | 11.78 | 22.55 |
| Logical bytes / file bytes | 5.77 | 3.02 |
| Write peak RSS (MiB) | 22.82 | 4.92 |
| Read peak RSS (MiB) | 73.90 | 8.78 |
| Capture time (ms) | 43.69 | 38.36 |
| Capture time (ns/tick) | 364.1 | 319.7 |
| File bytes per tick | 20.2 | 38.7 |
| Close time (ms) | 7.94 | 0.17 |
| Write CPU time (ms) | 52.85 | 39.46 |
| Write min / max (ms) | 51.77 / 52.62 | 38.26 / 39.51 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 26.523 | 3.485 |
| search-transaction: p95 (ms) | 40.094 | 4.321 |
| search-transaction: peak RSS (MiB) | 74.09 | 8.65 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 36.843 | 5.018 |
| find-transaction: p95 (ms) | 45.626 | 5.421 |
| find-transaction: peak RSS (MiB) | 73.96 | 8.79 |

### many-generators

Observations: 329,168. Null producer: 14.33 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 90.00 | 68.71 |
| Write throughput (Mobs/s) | 3.658 | 4.791 |
| Read time (ms) | 91.24 | 20.94 |
| Read throughput (Mobs/s) | 3.608 | 15.717 |
| File size (MiB) | 3.806 | 7.571 |
| Bytes per observation | 12.12 | 24.12 |
| Logical bytes / file bytes | 5.61 | 2.82 |
| Write peak RSS (MiB) | 25.40 | 6.02 |
| Read peak RSS (MiB) | 113.66 | 11.79 |
| Capture time (ms) | 84.14 | 67.94 |
| Capture time (ns/tick) | 3505.8 | 2830.8 |
| File bytes per tick | 166.3 | 330.8 |
| Close time (ms) | 5.72 | 0.70 |
| Write CPU time (ms) | 91.06 | 69.66 |
| Write min / max (ms) | 89.03 / 90.38 | 67.73 / 69.48 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 26.810 | 0.626 |
| search-transaction: p95 (ms) | 28.315 | 1.361 |
| search-transaction: peak RSS (MiB) | 53.38 | 11.86 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 55.983 | 1.350 |
| find-transaction: p95 (ms) | 65.916 | 1.512 |
| find-transaction: peak RSS (MiB) | 113.68 | 11.85 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 12.33 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 60.04 | 94.85 | 50.27 |
| Write throughput (Mobs/s) | 22.339 | 14.141 | 26.679 |
| Read time (ms) | 38.27 | 39.02 | 42.06 |
| Read throughput (Mobs/s) | 35.044 | 34.372 | 31.890 |
| File size (MiB) | 2.521 | 4.010 | 31.812 |
| Bytes per observation | 1.97 | 3.13 | 24.87 |
| Logical bytes / file bytes | 9.36 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 37.96 | 41.17 | 6.09 |
| Read peak RSS (MiB) | 36.48 | 9.28 | 6.16 |
| Capture time (ms) | 45.70 | 46.53 | 49.47 |
| Capture time (ns/tick) | 1904.0 | 1938.8 | 2061.2 |
| File bytes per tick | 110.1 | 175.2 | 1389.9 |
| Close time (ms) | 14.09 | 48.32 | 0.26 |
| Write CPU time (ms) | 61.04 | 95.92 | 51.29 |
| Write min / max (ms) | 59.99 / 60.21 | 94.80 / 104.77 | 49.38 / 51.05 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.083 | 0.233 | 12.391 |
| search-signal: p95 (ms) | 0.243 | 2.115 | 17.218 |
| search-signal: peak RSS (MiB) | 6.29 | 9.85 | 6.71 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.151 | 0.237 | 18.328 |
| find-signal: p95 (ms) | 1.204 | 2.050 | 18.759 |
| find-signal: peak RSS (MiB) | 7.83 | 9.82 | 6.70 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.074 | 0.037 | 12.371 |
| point: p95 (ms) | 0.223 | 0.162 | 17.548 |
| point: peak RSS (MiB) | 6.43 | 5.19 | 6.69 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 16.764 | 0.449 | 0.439 |
| search-transaction: p95 (ms) | 17.037 | 1.160 | 1.089 |
| search-transaction: peak RSS (MiB) | 34.80 | 6.06 | 6.09 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 17.617 | 1.097 | 1.092 |
| find-transaction: p95 (ms) | 17.946 | 1.166 | 1.185 |
| find-transaction: peak RSS (MiB) | 34.76 | 6.05 | 6.02 |

## Interpretation

FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
