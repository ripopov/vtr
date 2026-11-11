# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **74.19 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU and synchronous producers. Binary formats use LZ4, except the gpu-instructions-zstd profile, which uses Zstandard level 1. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 39.76 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 93.51 | 395.50 | 135.59 |
| Write throughput (Mobs/s) | 67.276 | 15.907 | 46.398 |
| Read time (ms) | 43.07 | 134.04 | 161.60 |
| Read throughput (Mobs/s) | 146.072 | 46.935 | 38.930 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 21.64 | 143.22 | 5.83 |
| Read peak RSS (MiB) | 14.30 | 26.13 | 4.64 |
| Capture time (ms) | 92.33 | 350.99 | 135.51 |
| Capture time (ns/tick) | 192.3 | 731.2 | 282.3 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.07 | 48.68 | 0.04 |
| Write CPU time (ms) | 94.00 | 396.03 | 136.06 |
| Write min / max (ms) | 93.20 / 103.70 | 392.23 / 401.54 | 133.81 / 138.95 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.249 | 1.094 | 56.850 |
| search-signal: p95 (ms) | 0.422 | 4.581 | 90.002 |
| search-signal: peak RSS (MiB) | 8.56 | 27.37 | 6.65 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 1.274 | 1.494 | 94.496 |
| find-signal: p95 (ms) | 3.667 | 6.116 | 98.672 |
| find-signal: peak RSS (MiB) | 11.08 | 27.43 | 7.12 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.251 | 0.354 | 56.872 |
| point: p95 (ms) | 0.747 | 0.935 | 89.948 |
| point: peak RSS (MiB) | 8.79 | 9.41 | 6.69 |

### soc

Observations: 20,143,793. Null producer: 113.53 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 371.74 | 654.01 | 409.89 |
| Write throughput (Mobs/s) | 54.188 | 30.800 | 49.144 |
| Read time (ms) | 132.93 | 451.87 | 503.45 |
| Read throughput (Mobs/s) | 151.532 | 44.578 | 40.011 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 17.27 | 147.53 | 4.73 |
| Read peak RSS (MiB) | 31.19 | 23.09 | 4.66 |
| Capture time (ms) | 365.17 | 634.69 | 409.33 |
| Capture time (ns/tick) | 15215.4 | 26445.4 | 17055.2 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 5.77 | 18.11 | 0.12 |
| Write CPU time (ms) | 372.33 | 654.47 | 410.35 |
| Write min / max (ms) | 370.54 / 408.43 | 645.85 / 657.62 | 408.46 / 409.93 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.298 | 0.229 | 176.965 |
| search-signal: p95 (ms) | 0.446 | 0.325 | 250.661 |
| search-signal: peak RSS (MiB) | 8.29 | 22.84 | 6.68 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.417 | 0.513 | 269.734 |
| find-signal: p95 (ms) | 0.593 | 0.768 | 301.939 |
| find-signal: peak RSS (MiB) | 11.96 | 27.14 | 6.79 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.296 | 0.129 | 177.854 |
| point: p95 (ms) | 0.415 | 0.194 | 248.737 |
| point: peak RSS (MiB) | 8.73 | 4.15 | 7.05 |

### idle

Observations: 2,547,072. Null producer: 14.43 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 53.10 | 73.32 | 53.36 |
| Write throughput (Mobs/s) | 47.971 | 34.737 | 47.732 |
| Read time (ms) | 17.77 | 56.60 | 64.94 |
| Read throughput (Mobs/s) | 143.310 | 44.999 | 39.222 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.52 | 79.95 | 4.97 |
| Read peak RSS (MiB) | 7.00 | 15.05 | 4.66 |
| Capture time (ms) | 45.55 | 29.64 | 52.82 |
| Capture time (ns/tick) | 949.0 | 617.4 | 1100.3 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 6.82 | 43.15 | 0.10 |
| Write CPU time (ms) | 53.61 | 73.77 | 53.79 |
| Write min / max (ms) | 51.34 / 55.66 | 72.55 / 73.81 | 53.07 / 54.50 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.309 | 0.148 | 13.648 |
| search-signal: p95 (ms) | 0.501 | 0.260 | 34.362 |
| search-signal: peak RSS (MiB) | 7.55 | 15.68 | 7.02 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.370 | 0.150 | 34.787 |
| find-signal: p95 (ms) | 0.407 | 0.232 | 35.678 |
| find-signal: peak RSS (MiB) | 7.65 | 15.97 | 6.78 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.311 | 0.096 | 14.461 |
| point: p95 (ms) | 0.497 | 0.194 | 33.603 |
| point: peak RSS (MiB) | 7.33 | 5.85 | 6.74 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.74 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 27.22 | 31.33 |
| Write throughput (Mobs/s) | 7.559 | 6.567 |
| Read time (ms) | 14.62 | 11.07 |
| Read throughput (Mobs/s) | 14.070 | 18.582 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.82 | 4.13 |
| Read peak RSS (MiB) | 5.32 | 7.67 |
| Capture time (ms) | 27.02 | 31.17 |
| Capture time (ns/tick) | 225.2 | 259.7 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.10 | 0.13 |
| Write CPU time (ms) | 27.64 | 31.71 |
| Write min / max (ms) | 27.19 / 28.44 | 31.28 / 31.68 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.083 | 3.516 |
| search-transaction: p95 (ms) | 0.122 | 4.503 |
| search-transaction: peak RSS (MiB) | 5.32 | 9.74 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 3.873 | 5.083 |
| find-transaction: p95 (ms) | 4.001 | 5.179 |
| find-transaction: peak RSS (MiB) | 5.32 | 7.72 |

### many-generators

Observations: 329,168. Null producer: 15.66 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 48.74 | 59.17 |
| Write throughput (Mobs/s) | 6.753 | 5.563 |
| Read time (ms) | 25.26 | 20.84 |
| Read throughput (Mobs/s) | 13.031 | 15.796 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.91 | 7.07 |
| Read peak RSS (MiB) | 7.98 | 10.87 |
| Capture time (ms) | 48.55 | 58.52 |
| Capture time (ns/tick) | 2022.8 | 2438.4 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.09 | 0.60 |
| Write CPU time (ms) | 49.14 | 59.56 |
| Write min / max (ms) | 48.53 / 50.79 | 58.80 / 60.99 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.187 | 0.673 |
| search-transaction: p95 (ms) | 0.242 | 1.463 |
| search-transaction: peak RSS (MiB) | 6.06 | 12.84 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 4.535 | 1.394 |
| find-transaction: p95 (ms) | 4.715 | 1.466 |
| find-transaction: peak RSS (MiB) | 6.06 | 12.90 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.42 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 31.35 | 54.00 | 41.53 |
| Write throughput (Mobs/s) | 42.779 | 24.839 | 32.300 |
| Read time (ms) | 14.62 | 31.61 | 40.88 |
| Read throughput (Mobs/s) | 91.731 | 42.438 | 32.811 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 16.34 | 42.18 | 7.24 |
| Read peak RSS (MiB) | 8.50 | 10.09 | 5.98 |
| Capture time (ms) | 28.69 | 29.53 | 41.10 |
| Capture time (ns/tick) | 1195.4 | 1230.4 | 1712.5 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 2.50 | 24.49 | 0.17 |
| Write CPU time (ms) | 31.83 | 54.43 | 41.97 |
| Write min / max (ms) | 31.04 / 32.57 | 52.86 / 54.17 | 41.27 / 41.94 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.109 | 0.114 | 14.002 |
| search-signal: p95 (ms) | 0.197 | 0.341 | 20.111 |
| search-signal: peak RSS (MiB) | 5.68 | 10.58 | 6.79 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.118 | 0.120 | 21.430 |
| find-signal: p95 (ms) | 0.252 | 0.337 | 22.497 |
| find-signal: peak RSS (MiB) | 5.68 | 10.77 | 7.39 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.108 | 0.044 | 13.978 |
| point: p95 (ms) | 0.191 | 0.091 | 20.169 |
| point: peak RSS (MiB) | 5.68 | 5.81 | 6.89 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.079 | 0.429 | 0.430 |
| search-transaction: p95 (ms) | 0.117 | 1.060 | 1.041 |
| search-transaction: peak RSS (MiB) | 4.90 | 7.09 | 4.98 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.413 | 1.053 | 1.046 |
| find-transaction: p95 (ms) | 1.573 | 1.101 | 1.092 |
| find-transaction: peak RSS (MiB) | 6.82 | 5.09 | 7.10 |

## GPU instruction descriptions

### gpu-instructions

Observations: 262,144. Null producer: 0.42 ms.

| Metric | VTR-PAYLOAD | VTR-INTERNED | VTR-PAYLOAD-BATCHED | VTR-INTERNED-BATCHED | VTR-PAYLOAD-FIXED | VTR-INTERNED-FIXED |
|---|---:|---:|---:|---:|---:|---:|
| Write time (ms) | 98.43 | 90.58 | 142.71 | 131.14 | 98.54 | 133.85 |
| Write throughput (Mobs/s) | 2.663 | 2.894 | 1.837 | 1.999 | 2.660 | 1.958 |
| Read time (ms) | 563.81 | 521.05 | 562.67 | 506.08 | 565.68 | 503.63 |
| Read throughput (Mobs/s) | 0.465 | 0.503 | 0.466 | 0.518 | 0.463 | 0.521 |
| File size (MiB) | 3.736 | 3.754 | 3.438 | 3.541 | 3.630 | 3.790 |
| Bytes per observation | 14.94 | 15.02 | 13.75 | 14.16 | 14.52 | 15.16 |
| Logical bytes / file bytes | 18.30 | 18.21 | 19.88 | 19.30 | 18.83 | 18.03 |
| Write peak RSS (MiB) | 7.98 | 6.05 | 10.06 | 10.02 | 7.15 | 8.07 |
| Read peak RSS (MiB) | 8.70 | 10.38 | 8.21 | 10.18 | 10.47 | 8.62 |
| Capture time (ms) | 98.10 | 90.27 | 141.98 | 130.03 | 98.37 | 133.65 |
| Capture time (ns/tick) | 374.2 | 344.3 | 541.6 | 496.0 | 375.3 | 509.8 |
| File bytes per tick | 14.9 | 15.0 | 13.8 | 14.2 | 14.5 | 15.2 |
| Close time (ms) | 0.14 | 0.12 | 0.52 | 0.89 | 0.02 | 0.02 |
| Write CPU time (ms) | 99.02 | 90.98 | 143.27 | 131.44 | 99.13 | 134.25 |
| Write min / max (ms) | 97.77 / 121.52 | 90.36 / 91.96 | 140.43 / 142.72 | 131.06 / 136.06 | 96.12 / 142.29 | 89.86 / 137.36 |
| Transaction blocks | 306 | 324 | 77 | 81 | 257 | 257 |
| Peak transaction buffer (KiB) | 512.56 | 512.60 | 2048.56 | 2048.50 | 614.00 | 650.00 |

### gpu-instructions-zstd

Observations: 262,144. Null producer: 0.44 ms.

| Metric | VTR-PAYLOAD | VTR-INTERNED | VTR-PAYLOAD-BATCHED | VTR-INTERNED-BATCHED | VTR-PAYLOAD-FIXED | VTR-INTERNED-FIXED |
|---|---:|---:|---:|---:|---:|---:|
| Write time (ms) | 153.84 | 139.78 | 147.82 | 135.80 | 151.10 | 139.88 |
| Write throughput (Mobs/s) | 1.704 | 1.875 | 1.773 | 1.930 | 1.735 | 1.874 |
| Read time (ms) | 709.54 | 644.24 | 653.74 | 608.94 | 694.85 | 631.28 |
| Read throughput (Mobs/s) | 0.369 | 0.407 | 0.401 | 0.430 | 0.377 | 0.415 |
| File size (MiB) | 2.359 | 2.313 | 2.084 | 2.024 | 2.273 | 2.194 |
| Bytes per observation | 9.44 | 9.25 | 8.34 | 8.10 | 9.09 | 8.78 |
| Logical bytes / file bytes | 28.97 | 29.55 | 32.80 | 33.77 | 30.08 | 31.15 |
| Write peak RSS (MiB) | 7.92 | 8.05 | 10.35 | 10.22 | 8.32 | 8.37 |
| Read peak RSS (MiB) | 9.63 | 8.91 | 9.06 | 8.83 | 8.84 | 9.39 |
| Capture time (ms) | 153.38 | 139.33 | 147.00 | 134.66 | 150.90 | 139.64 |
| Capture time (ns/tick) | 585.1 | 531.5 | 560.8 | 513.7 | 575.6 | 532.7 |
| File bytes per tick | 9.4 | 9.3 | 8.3 | 8.1 | 9.1 | 8.8 |
| Close time (ms) | 0.24 | 0.21 | 0.59 | 0.96 | 0.03 | 0.03 |
| Write CPU time (ms) | 154.38 | 140.35 | 148.34 | 136.16 | 151.62 | 140.47 |
| Write min / max (ms) | 131.56 / 157.58 | 134.07 / 145.06 | 146.03 / 155.76 | 133.93 / 138.04 | 106.73 / 153.81 | 96.79 / 141.00 |
| Transaction blocks | 306 | 324 | 77 | 81 | 257 | 257 |
| Peak transaction buffer (KiB) | 512.56 | 512.60 | 2048.56 | 2048.50 | 614.00 | 650.00 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation in the common-format workloads.

The VTR-only gpu-instructions profile compares identical typed instruction content in canonical payload fields and in 50 shared descriptions referenced by an attribute. A short loop runs across 128 wave sources; each instruction has five embedded phases, dynamic execution masks, memory-phase attributes and a wave ownership relation. Both representations retain the viewer label. Default variants use the production buffer limits. Fixed variants use a 2,048-item block limit and a nonbinding byte limit to isolate compression from memory-based batching. Batched variants use a 2 MiB block limit; independently decoded pages remain bounded by the default 64 KiB target. Reads check instruction content, IDs, ownership, times, phases and all 128 waves' relationships against the generated schedule. Its null baseline measures schedule checksumming only and does not model phase capture costs.
