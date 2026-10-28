# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **52.31 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 47.83 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 109.82 | 647.91 | 160.83 |
| Write throughput (Mobs/s) | 57.283 | 9.710 | 39.116 |
| Read time (ms) | 39.05 | 167.52 | 180.23 |
| Read throughput (Mobs/s) | 161.114 | 37.553 | 34.907 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 18.85 | 141.03 | 5.56 |
| Read peak RSS (MiB) | 14.07 | 26.25 | 5.22 |
| Capture time (ms) | 108.27 | 555.15 | 160.21 |
| Capture time (ns/tick) | 225.6 | 1156.6 | 333.8 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.44 | 92.04 | 0.11 |
| Write CPU time (ms) | 110.73 | 648.59 | 161.59 |
| Write min / max (ms) | 109.56 / 110.84 | 631.42 / 653.64 | 160.40 / 162.16 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.112 | 2.403 | 55.151 |
| search-signal: p95 (ms) | 0.419 | 6.214 | 88.737 |
| search-signal: peak RSS (MiB) | 7.64 | 26.98 | 6.61 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.630 | 2.884 | 92.979 |
| find-signal: p95 (ms) | 2.695 | 8.477 | 96.740 |
| find-signal: peak RSS (MiB) | 10.20 | 26.96 | 6.67 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.120 | 0.359 | 55.932 |
| point: p95 (ms) | 0.474 | 1.368 | 89.842 |
| point: peak RSS (MiB) | 7.68 | 8.94 | 6.63 |

### soc

Observations: 20,143,793. Null producer: 139.80 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 412.63 | 828.04 | 496.99 |
| Write throughput (Mobs/s) | 48.818 | 24.327 | 40.532 |
| Read time (ms) | 120.36 | 587.51 | 567.06 |
| Read throughput (Mobs/s) | 167.359 | 34.287 | 35.523 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 16.20 | 145.58 | 5.61 |
| Read peak RSS (MiB) | 30.67 | 22.70 | 5.42 |
| Capture time (ms) | 404.85 | 803.37 | 496.11 |
| Capture time (ns/tick) | 16868.6 | 33473.8 | 20671.3 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 6.51 | 23.96 | 0.16 |
| Write CPU time (ms) | 413.36 | 827.67 | 497.37 |
| Write min / max (ms) | 409.73 / 416.07 | 820.00 / 835.57 | 491.67 / 498.16 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.415 | 0.410 | 181.478 |
| search-signal: p95 (ms) | 0.664 | 6.486 | 257.360 |
| search-signal: peak RSS (MiB) | 7.00 | 22.67 | 6.68 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.570 | 7.036 | 276.383 |
| find-signal: p95 (ms) | 0.796 | 7.430 | 280.742 |
| find-signal: peak RSS (MiB) | 9.36 | 22.41 | 6.62 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.414 | 0.195 | 180.609 |
| point: p95 (ms) | 0.612 | 0.299 | 257.473 |
| point: peak RSS (MiB) | 7.07 | 5.08 | 6.67 |

### idle

Observations: 2,547,072. Null producer: 17.59 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 56.54 | 113.92 | 63.48 |
| Write throughput (Mobs/s) | 45.046 | 22.358 | 40.125 |
| Read time (ms) | 16.83 | 73.04 | 71.59 |
| Read throughput (Mobs/s) | 151.355 | 34.870 | 35.581 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.39 | 77.54 | 5.73 |
| Read peak RSS (MiB) | 7.71 | 14.74 | 5.26 |
| Capture time (ms) | 47.74 | 58.47 | 62.68 |
| Capture time (ns/tick) | 994.6 | 1218.0 | 1305.8 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 7.54 | 55.02 | 0.14 |
| Write CPU time (ms) | 57.49 | 114.66 | 64.11 |
| Write min / max (ms) | 55.87 / 56.93 | 113.50 / 114.16 | 63.36 / 63.90 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.389 | 0.327 | 13.488 |
| search-signal: p95 (ms) | 0.636 | 3.995 | 33.556 |
| search-signal: peak RSS (MiB) | 6.75 | 15.08 | 6.68 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.413 | 0.328 | 34.369 |
| find-signal: p95 (ms) | 0.678 | 3.711 | 34.942 |
| find-signal: peak RSS (MiB) | 6.66 | 15.12 | 6.67 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.395 | 0.094 | 13.147 |
| point: p95 (ms) | 0.620 | 0.279 | 32.936 |
| point: peak RSS (MiB) | 6.77 | 5.48 | 6.68 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.18 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 28.19 | 37.47 |
| Write throughput (Mobs/s) | 7.297 | 5.491 |
| Read time (ms) | 15.24 | 11.29 |
| Read throughput (Mobs/s) | 13.498 | 18.215 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.32 | 5.01 |
| Read peak RSS (MiB) | 6.23 | 8.85 |
| Capture time (ms) | 27.91 | 37.25 |
| Capture time (ns/tick) | 232.6 | 310.4 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.12 | 0.17 |
| Write CPU time (ms) | 29.05 | 38.16 |
| Write min / max (ms) | 28.14 / 28.38 | 36.71 / 38.28 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.075 | 3.406 |
| search-transaction: p95 (ms) | 0.162 | 4.202 |
| search-transaction: peak RSS (MiB) | 6.25 | 8.79 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 4.737 | 4.883 |
| find-transaction: p95 (ms) | 4.947 | 5.208 |
| find-transaction: peak RSS (MiB) | 6.18 | 8.79 |

### many-generators

Observations: 329,168. Null producer: 13.91 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 51.78 | 65.94 |
| Write throughput (Mobs/s) | 6.357 | 4.992 |
| Read time (ms) | 25.11 | 20.77 |
| Read throughput (Mobs/s) | 13.107 | 15.851 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.50 | 6.22 |
| Read peak RSS (MiB) | 7.04 | 11.93 |
| Capture time (ms) | 51.47 | 65.23 |
| Capture time (ns/tick) | 2144.5 | 2717.9 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.13 | 0.67 |
| Write CPU time (ms) | 51.98 | 66.68 |
| Write min / max (ms) | 51.22 / 59.91 | 64.95 / 65.96 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.167 | 0.597 |
| search-transaction: p95 (ms) | 0.277 | 1.350 |
| search-transaction: peak RSS (MiB) | 7.06 | 11.95 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 5.521 | 1.304 |
| find-transaction: p95 (ms) | 5.684 | 1.358 |
| find-transaction: peak RSS (MiB) | 7.01 | 12.04 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.91 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 36.60 | 84.46 | 48.28 |
| Write throughput (Mobs/s) | 36.643 | 15.880 | 27.781 |
| Read time (ms) | 13.88 | 38.16 | 40.92 |
| Read throughput (Mobs/s) | 96.628 | 35.153 | 32.778 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.83 | 41.08 | 6.10 |
| Read peak RSS (MiB) | 7.36 | 9.48 | 6.09 |
| Capture time (ms) | 32.11 | 44.90 | 47.56 |
| Capture time (ns/tick) | 1337.9 | 1870.6 | 1981.7 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 4.20 | 39.39 | 0.24 |
| Write CPU time (ms) | 37.37 | 85.18 | 49.03 |
| Write min / max (ms) | 36.23 / 36.79 | 84.35 / 86.29 | 48.05 / 48.44 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.081 | 0.224 | 11.878 |
| search-signal: p95 (ms) | 0.244 | 1.949 | 16.796 |
| search-signal: peak RSS (MiB) | 6.61 | 9.82 | 6.66 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.094 | 0.207 | 17.882 |
| find-signal: p95 (ms) | 0.258 | 1.749 | 18.323 |
| find-signal: peak RSS (MiB) | 6.57 | 9.84 | 6.67 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.085 | 0.036 | 11.820 |
| point: p95 (ms) | 0.222 | 0.145 | 16.405 |
| point: peak RSS (MiB) | 6.61 | 5.19 | 6.67 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.083 | 0.445 | 0.435 |
| search-transaction: p95 (ms) | 0.162 | 1.093 | 1.077 |
| search-transaction: peak RSS (MiB) | 5.83 | 6.18 | 6.18 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.784 | 1.073 | 1.074 |
| find-transaction: p95 (ms) | 1.885 | 1.143 | 1.122 |
| find-transaction: peak RSS (MiB) | 5.81 | 6.16 | 6.17 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
