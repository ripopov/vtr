# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **49.07 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 43.94 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 96.92 | 424.61 | 149.66 |
| Write throughput (Mobs/s) | 64.910 | 14.816 | 42.035 |
| Read time (ms) | 44.13 | 135.54 | 167.19 |
| Read throughput (Mobs/s) | 142.562 | 46.415 | 37.628 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 21.56 | 143.32 | 4.65 |
| Read peak RSS (MiB) | 14.23 | 25.82 | 4.64 |
| Capture time (ms) | 95.52 | 373.42 | 149.34 |
| Capture time (ns/tick) | 199.0 | 778.0 | 311.1 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.29 | 49.35 | 0.04 |
| Write CPU time (ms) | 97.33 | 425.08 | 150.05 |
| Write min / max (ms) | 95.82 / 103.30 | 417.98 / 425.93 | 141.25 / 153.50 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.255 | 1.139 | 57.619 |
| search-signal: p95 (ms) | 0.434 | 4.719 | 90.998 |
| search-signal: peak RSS (MiB) | 8.20 | 27.46 | 7.21 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 1.369 | 1.520 | 95.162 |
| find-signal: p95 (ms) | 3.801 | 6.462 | 102.096 |
| find-signal: peak RSS (MiB) | 10.27 | 28.00 | 7.07 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.239 | 0.356 | 58.791 |
| point: p95 (ms) | 0.669 | 0.764 | 92.720 |
| point: peak RSS (MiB) | 8.76 | 8.93 | 7.34 |

### soc

Observations: 20,143,793. Null producer: 111.66 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 385.17 | 665.31 | 420.04 |
| Write throughput (Mobs/s) | 52.298 | 30.277 | 47.957 |
| Read time (ms) | 134.09 | 451.93 | 512.08 |
| Read throughput (Mobs/s) | 150.223 | 44.572 | 39.337 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 17.54 | 147.92 | 4.90 |
| Read peak RSS (MiB) | 29.73 | 23.53 | 4.64 |
| Capture time (ms) | 378.10 | 646.72 | 419.55 |
| Capture time (ns/tick) | 15754.0 | 26946.6 | 17481.4 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 6.23 | 17.96 | 0.06 |
| Write CPU time (ms) | 385.55 | 665.71 | 420.42 |
| Write min / max (ms) | 372.32 / 397.78 | 656.90 / 676.14 | 416.06 / 424.97 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.316 | 0.232 | 178.905 |
| search-signal: p95 (ms) | 0.488 | 0.310 | 252.911 |
| search-signal: peak RSS (MiB) | 8.70 | 22.93 | 7.30 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.439 | 0.507 | 274.352 |
| find-signal: p95 (ms) | 0.599 | 0.758 | 282.793 |
| find-signal: peak RSS (MiB) | 12.33 | 27.36 | 6.77 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.293 | 0.133 | 179.330 |
| point: p95 (ms) | 0.431 | 0.201 | 256.881 |
| point: peak RSS (MiB) | 8.77 | 6.03 | 7.23 |

### idle

Observations: 2,547,072. Null producer: 15.56 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 53.30 | 75.41 | 53.76 |
| Write throughput (Mobs/s) | 47.785 | 33.776 | 47.379 |
| Read time (ms) | 18.07 | 56.71 | 65.93 |
| Read throughput (Mobs/s) | 140.975 | 44.916 | 38.631 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.63 | 78.75 | 4.90 |
| Read peak RSS (MiB) | 8.84 | 13.72 | 4.65 |
| Capture time (ms) | 45.59 | 31.17 | 53.27 |
| Capture time (ns/tick) | 949.8 | 649.5 | 1109.8 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 6.97 | 43.50 | 0.06 |
| Write CPU time (ms) | 53.70 | 75.81 | 54.13 |
| Write min / max (ms) | 53.08 / 53.96 | 73.19 / 75.73 | 53.75 / 54.93 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.311 | 0.158 | 14.317 |
| search-signal: p95 (ms) | 0.510 | 0.230 | 37.441 |
| search-signal: peak RSS (MiB) | 7.49 | 16.00 | 6.61 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.349 | 0.153 | 35.935 |
| find-signal: p95 (ms) | 0.521 | 0.254 | 46.720 |
| find-signal: peak RSS (MiB) | 7.80 | 16.08 | 6.71 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.289 | 0.102 | 14.124 |
| point: p95 (ms) | 0.512 | 0.202 | 37.921 |
| point: peak RSS (MiB) | 7.50 | 6.12 | 6.43 |

## Transaction results

### requests

Observations: 205,728. Null producer: 9.71 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 31.76 | 47.07 |
| Write throughput (Mobs/s) | 6.477 | 4.370 |
| Read time (ms) | 19.49 | 13.95 |
| Read throughput (Mobs/s) | 10.557 | 14.743 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.59 | 5.87 |
| Read peak RSS (MiB) | 5.32 | 9.71 |
| Capture time (ms) | 31.33 | 46.86 |
| Capture time (ns/tick) | 261.1 | 390.5 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.14 | 0.15 |
| Write CPU time (ms) | 32.34 | 47.62 |
| Write min / max (ms) | 30.16 / 36.26 | 34.85 / 49.54 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.096 | 3.906 |
| search-transaction: p95 (ms) | 0.251 | 5.881 |
| search-transaction: peak RSS (MiB) | 7.33 | 9.65 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 4.534 | 5.630 |
| find-transaction: p95 (ms) | 6.552 | 7.352 |
| find-transaction: peak RSS (MiB) | 5.32 | 7.78 |

### many-generators

Observations: 329,168. Null producer: 17.72 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 58.80 | 73.68 |
| Write throughput (Mobs/s) | 5.598 | 4.467 |
| Read time (ms) | 31.33 | 26.13 |
| Read throughput (Mobs/s) | 10.506 | 12.598 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 5.77 | 6.45 |
| Read peak RSS (MiB) | 6.06 | 10.93 |
| Capture time (ms) | 58.46 | 72.98 |
| Capture time (ns/tick) | 2436.0 | 3041.0 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.16 | 0.65 |
| Write CPU time (ms) | 59.31 | 74.17 |
| Write min / max (ms) | 58.62 / 65.54 | 71.04 / 76.51 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.263 | 0.994 |
| search-transaction: p95 (ms) | 0.516 | 2.344 |
| search-transaction: peak RSS (MiB) | 6.06 | 12.85 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 5.230 | 1.648 |
| find-transaction: p95 (ms) | 8.906 | 3.003 |
| find-transaction: peak RSS (MiB) | 6.06 | 10.87 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.61 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 32.76 | 58.91 | 43.18 |
| Write throughput (Mobs/s) | 40.948 | 22.766 | 31.065 |
| Read time (ms) | 15.00 | 32.36 | 42.31 |
| Read throughput (Mobs/s) | 89.396 | 41.446 | 31.699 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.79 | 44.16 | 7.23 |
| Read peak RSS (MiB) | 8.49 | 8.53 | 5.96 |
| Capture time (ms) | 29.79 | 29.42 | 42.76 |
| Capture time (ns/tick) | 1241.1 | 1226.0 | 1781.7 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 2.68 | 29.29 | 0.24 |
| Write CPU time (ms) | 33.26 | 59.29 | 43.64 |
| Write min / max (ms) | 31.29 / 33.07 | 55.06 / 88.21 | 42.56 / 44.96 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.110 | 0.166 | 14.419 |
| search-signal: p95 (ms) | 0.225 | 0.585 | 20.568 |
| search-signal: peak RSS (MiB) | 5.68 | 10.81 | 6.62 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.141 | 0.138 | 21.955 |
| find-signal: p95 (ms) | 0.287 | 0.445 | 28.296 |
| find-signal: peak RSS (MiB) | 7.57 | 9.88 | 7.16 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.115 | 0.039 | 15.214 |
| point: p95 (ms) | 0.207 | 0.105 | 21.294 |
| point: peak RSS (MiB) | 5.68 | 5.89 | 7.09 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.081 | 0.447 | 0.440 |
| search-transaction: p95 (ms) | 0.139 | 1.088 | 1.087 |
| search-transaction: peak RSS (MiB) | 4.98 | 5.10 | 5.10 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.437 | 1.091 | 1.079 |
| find-transaction: p95 (ms) | 1.624 | 1.318 | 1.161 |
| find-transaction: peak RSS (MiB) | 4.99 | 5.10 | 5.10 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
