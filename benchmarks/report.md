# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **56.80 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 47.36 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 104.71 | 633.12 | 146.34 |
| Write throughput (Mobs/s) | 60.081 | 9.937 | 42.989 |
| Read time (ms) | 48.73 | 158.71 | 178.27 |
| Read throughput (Mobs/s) | 129.091 | 39.639 | 35.290 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 21.32 | 140.57 | 4.59 |
| Read peak RSS (MiB) | 13.93 | 25.68 | 5.71 |
| Capture time (ms) | 103.15 | 504.01 | 146.00 |
| Capture time (ns/tick) | 214.9 | 1050.0 | 304.2 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.42 | 98.73 | 0.09 |
| Write CPU time (ms) | 105.26 | 614.60 | 146.88 |
| Write min / max (ms) | 99.79 / 112.34 | 531.88 / 633.80 | 144.08 / 189.73 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.260 | 1.267 | 64.868 |
| search-signal: p95 (ms) | 0.514 | 5.527 | 109.992 |
| search-signal: peak RSS (MiB) | 8.34 | 27.71 | 6.58 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 1.563 | 2.043 | 108.605 |
| find-signal: p95 (ms) | 4.218 | 9.051 | 143.955 |
| find-signal: peak RSS (MiB) | 10.54 | 27.01 | 6.47 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.297 | 0.393 | 60.466 |
| point: p95 (ms) | 1.686 | 1.234 | 108.578 |
| point: peak RSS (MiB) | 7.98 | 9.07 | 6.83 |

### soc

Observations: 20,143,793. Null producer: 135.12 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 454.29 | 770.42 | 487.50 |
| Write throughput (Mobs/s) | 44.341 | 26.147 | 41.320 |
| Read time (ms) | 145.09 | 539.45 | 607.79 |
| Read throughput (Mobs/s) | 138.836 | 37.341 | 33.143 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 17.63 | 147.21 | 4.84 |
| Read peak RSS (MiB) | 31.68 | 23.56 | 4.59 |
| Capture time (ms) | 446.52 | 747.17 | 486.85 |
| Capture time (ns/tick) | 18604.9 | 31132.2 | 20285.2 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 7.04 | 21.92 | 0.08 |
| Write CPU time (ms) | 454.95 | 771.06 | 483.32 |
| Write min / max (ms) | 435.12 / 467.36 | 751.06 / 889.05 | 482.74 / 548.92 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.356 | 0.336 | 199.093 |
| search-signal: p95 (ms) | 0.540 | 0.644 | 349.645 |
| search-signal: peak RSS (MiB) | 9.11 | 23.02 | 6.73 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.511 | 0.726 | 314.927 |
| find-signal: p95 (ms) | 0.739 | 1.908 | 411.943 |
| find-signal: peak RSS (MiB) | 11.83 | 26.57 | 6.76 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.342 | 0.150 | 199.408 |
| point: p95 (ms) | 0.593 | 0.311 | 307.497 |
| point: peak RSS (MiB) | 8.66 | 4.09 | 5.80 |

### idle

Observations: 2,547,072. Null producer: 15.87 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 59.69 | 79.58 | 57.58 |
| Write throughput (Mobs/s) | 42.669 | 32.005 | 44.235 |
| Read time (ms) | 19.48 | 62.81 | 68.06 |
| Read throughput (Mobs/s) | 130.769 | 40.549 | 37.424 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 16.33 | 78.42 | 4.84 |
| Read peak RSS (MiB) | 6.94 | 13.79 | 4.59 |
| Capture time (ms) | 51.76 | 29.80 | 57.05 |
| Capture time (ns/tick) | 1078.3 | 620.8 | 1188.6 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 7.67 | 49.19 | 0.11 |
| Write CPU time (ms) | 60.16 | 80.07 | 58.10 |
| Write min / max (ms) | 59.25 / 62.43 | 79.40 / 80.15 | 57.28 / 58.97 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.342 | 0.186 | 15.112 |
| search-signal: p95 (ms) | 0.567 | 0.354 | 37.089 |
| search-signal: peak RSS (MiB) | 7.67 | 16.02 | 6.70 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.380 | 0.171 | 37.903 |
| find-signal: p95 (ms) | 0.594 | 0.266 | 38.558 |
| find-signal: peak RSS (MiB) | 7.69 | 15.92 | 6.66 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.348 | 0.115 | 15.167 |
| point: p95 (ms) | 0.547 | 0.227 | 37.209 |
| point: peak RSS (MiB) | 7.50 | 6.07 | 7.27 |

## Transaction results

### requests

Observations: 205,728. Null producer: 8.58 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 29.83 | 60.92 |
| Write throughput (Mobs/s) | 6.896 | 3.377 |
| Read time (ms) | 18.19 | 15.12 |
| Read throughput (Mobs/s) | 11.309 | 13.607 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 5.91 | 5.73 |
| Read peak RSS (MiB) | 7.22 | 7.67 |
| Capture time (ms) | 29.56 | 60.65 |
| Capture time (ns/tick) | 246.3 | 505.4 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.13 | 0.16 |
| Write CPU time (ms) | 30.40 | 61.90 |
| Write min / max (ms) | 29.60 / 46.03 | 34.35 / 63.49 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.097 | 4.145 |
| search-transaction: p95 (ms) | 0.222 | 7.505 |
| search-transaction: peak RSS (MiB) | 7.25 | 7.61 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 4.777 | 5.521 |
| find-transaction: p95 (ms) | 7.853 | 10.524 |
| find-transaction: peak RSS (MiB) | 5.25 | 9.67 |

### many-generators

Observations: 329,168. Null producer: 16.57 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 52.92 | 63.29 |
| Write throughput (Mobs/s) | 6.220 | 5.201 |
| Read time (ms) | 28.19 | 21.41 |
| Read throughput (Mobs/s) | 11.678 | 15.373 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.59 | 6.78 |
| Read peak RSS (MiB) | 5.93 | 12.80 |
| Capture time (ms) | 52.71 | 62.58 |
| Capture time (ns/tick) | 2196.3 | 2607.6 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.11 | 0.66 |
| Write CPU time (ms) | 53.44 | 63.80 |
| Write min / max (ms) | 50.58 / 53.12 | 61.35 / 64.21 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.213 | 0.739 |
| search-transaction: p95 (ms) | 0.304 | 1.542 |
| search-transaction: peak RSS (MiB) | 7.91 | 12.80 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 5.849 | 1.486 |
| find-transaction: p95 (ms) | 6.872 | 1.559 |
| find-transaction: peak RSS (MiB) | 6.00 | 12.84 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.95 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 33.68 | 63.17 | 44.29 |
| Write throughput (Mobs/s) | 39.822 | 21.231 | 30.286 |
| Read time (ms) | 15.52 | 34.60 | 42.86 |
| Read throughput (Mobs/s) | 86.438 | 38.763 | 31.293 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 17.18 | 43.87 | 6.93 |
| Read peak RSS (MiB) | 8.12 | 10.41 | 5.98 |
| Capture time (ms) | 30.39 | 30.80 | 43.77 |
| Capture time (ns/tick) | 1266.3 | 1283.3 | 1823.9 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 3.04 | 32.25 | 0.27 |
| Write CPU time (ms) | 34.19 | 63.76 | 44.77 |
| Write min / max (ms) | 33.01 / 34.20 | 63.14 / 64.88 | 43.29 / 50.81 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.114 | 0.113 | 15.973 |
| search-signal: p95 (ms) | 0.247 | 0.365 | 23.521 |
| search-signal: peak RSS (MiB) | 5.62 | 10.10 | 6.62 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.124 | 0.125 | 23.583 |
| find-signal: p95 (ms) | 0.263 | 0.373 | 25.330 |
| find-signal: peak RSS (MiB) | 5.62 | 10.38 | 5.70 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.118 | 0.041 | 15.175 |
| point: p95 (ms) | 0.238 | 0.105 | 21.652 |
| point: peak RSS (MiB) | 5.62 | 4.12 | 6.79 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.090 | 0.466 | 0.462 |
| search-transaction: p95 (ms) | 0.143 | 1.106 | 1.086 |
| search-transaction: peak RSS (MiB) | 6.69 | 7.03 | 6.91 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.730 | 1.116 | 1.106 |
| find-transaction: p95 (ms) | 2.186 | 1.198 | 1.187 |
| find-transaction: peak RSS (MiB) | 6.80 | 4.92 | 7.02 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
