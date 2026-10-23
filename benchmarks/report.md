# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **52.45 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 48.29 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 110.23 | 608.28 | 167.37 |
| Write throughput (Mobs/s) | 57.074 | 10.342 | 37.588 |
| Read time (ms) | 39.07 | 166.03 | 183.22 |
| Read throughput (Mobs/s) | 161.012 | 37.891 | 34.336 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 18.98 | 141.01 | 5.61 |
| Read peak RSS (MiB) | 14.09 | 26.34 | 5.33 |
| Capture time (ms) | 108.72 | 536.00 | 166.77 |
| Capture time (ns/tick) | 226.5 | 1116.7 | 347.4 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.35 | 74.29 | 0.11 |
| Write CPU time (ms) | 110.89 | 608.95 | 168.03 |
| Write min / max (ms) | 108.68 / 112.94 | 587.51 / 826.11 | 164.24 / 168.46 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.114 | 2.420 | 55.969 |
| search-signal: p95 (ms) | 0.399 | 6.068 | 89.365 |
| search-signal: peak RSS (MiB) | 7.79 | 26.99 | 6.68 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.622 | 2.895 | 92.434 |
| find-signal: p95 (ms) | 2.697 | 8.367 | 97.043 |
| find-signal: peak RSS (MiB) | 10.18 | 26.97 | 6.65 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.119 | 0.358 | 54.167 |
| point: p95 (ms) | 0.462 | 1.237 | 86.658 |
| point: peak RSS (MiB) | 7.62 | 8.96 | 6.65 |

### soc

Observations: 20,143,793. Null producer: 136.97 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 386.51 | 804.40 | 500.67 |
| Write throughput (Mobs/s) | 52.117 | 25.042 | 40.234 |
| Read time (ms) | 118.20 | 576.91 | 568.74 |
| Read throughput (Mobs/s) | 170.419 | 34.917 | 35.418 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 16.27 | 145.50 | 5.72 |
| Read peak RSS (MiB) | 30.71 | 22.67 | 5.26 |
| Capture time (ms) | 379.25 | 780.61 | 499.78 |
| Capture time (ns/tick) | 15802.0 | 32525.3 | 20824.2 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 6.05 | 23.25 | 0.16 |
| Write CPU time (ms) | 387.07 | 804.58 | 501.16 |
| Write min / max (ms) | 383.72 / 387.41 | 803.92 / 825.48 | 497.21 / 506.12 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.429 | 0.398 | 180.950 |
| search-signal: p95 (ms) | 0.597 | 5.995 | 260.039 |
| search-signal: peak RSS (MiB) | 7.10 | 22.72 | 6.64 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.548 | 6.744 | 277.821 |
| find-signal: p95 (ms) | 0.727 | 7.276 | 283.315 |
| find-signal: peak RSS (MiB) | 9.44 | 22.36 | 6.66 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.424 | 0.190 | 176.566 |
| point: p95 (ms) | 0.562 | 0.283 | 252.144 |
| point: peak RSS (MiB) | 7.01 | 5.11 | 6.65 |

### idle

Observations: 2,547,072. Null producer: 17.25 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 54.83 | 109.58 | 63.07 |
| Write throughput (Mobs/s) | 46.458 | 23.245 | 40.384 |
| Read time (ms) | 16.59 | 71.95 | 70.19 |
| Read throughput (Mobs/s) | 153.545 | 35.400 | 36.286 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.43 | 77.52 | 5.70 |
| Read peak RSS (MiB) | 8.01 | 14.76 | 5.27 |
| Capture time (ms) | 46.92 | 56.09 | 62.23 |
| Capture time (ns/tick) | 977.4 | 1168.5 | 1296.4 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 6.87 | 53.00 | 0.14 |
| Write CPU time (ms) | 55.35 | 110.17 | 63.63 |
| Write min / max (ms) | 54.00 / 56.14 | 109.40 / 112.55 | 62.65 / 65.98 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.389 | 0.325 | 13.498 |
| search-signal: p95 (ms) | 0.605 | 3.619 | 32.592 |
| search-signal: peak RSS (MiB) | 6.79 | 15.11 | 6.66 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.410 | 0.321 | 34.008 |
| find-signal: p95 (ms) | 0.634 | 3.448 | 37.383 |
| find-signal: peak RSS (MiB) | 6.80 | 15.12 | 6.70 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.393 | 0.094 | 12.832 |
| point: p95 (ms) | 0.594 | 0.270 | 32.199 |
| point: peak RSS (MiB) | 6.78 | 5.45 | 6.70 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.19 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 28.47 | 37.04 |
| Write throughput (Mobs/s) | 7.227 | 5.554 |
| Read time (ms) | 21.55 | 11.14 |
| Read throughput (Mobs/s) | 9.546 | 18.471 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.30 | 4.95 |
| Read peak RSS (MiB) | 6.14 | 8.84 |
| Capture time (ms) | 28.18 | 36.85 |
| Capture time (ns/tick) | 234.9 | 307.1 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.11 | 0.15 |
| Write CPU time (ms) | 29.37 | 37.63 |
| Write min / max (ms) | 27.85 / 28.71 | 35.60 / 38.12 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.108 | 3.337 |
| search-transaction: p95 (ms) | 0.168 | 4.256 |
| search-transaction: peak RSS (MiB) | 6.27 | 8.80 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 8.755 | 4.855 |
| find-transaction: p95 (ms) | 10.450 | 5.023 |
| find-transaction: peak RSS (MiB) | 6.26 | 8.79 |

### many-generators

Observations: 329,168. Null producer: 13.61 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 50.42 | 64.44 |
| Write throughput (Mobs/s) | 6.529 | 5.109 |
| Read time (ms) | 36.34 | 19.92 |
| Read throughput (Mobs/s) | 9.057 | 16.526 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.48 | 6.05 |
| Read peak RSS (MiB) | 7.05 | 11.88 |
| Capture time (ms) | 50.15 | 63.78 |
| Capture time (ns/tick) | 2089.5 | 2657.6 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.12 | 0.63 |
| Write CPU time (ms) | 51.00 | 65.02 |
| Write min / max (ms) | 50.03 / 50.80 | 64.14 / 66.13 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.292 | 0.578 |
| search-transaction: p95 (ms) | 0.384 | 1.309 |
| search-transaction: peak RSS (MiB) | 7.11 | 12.00 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 12.325 | 1.247 |
| find-transaction: p95 (ms) | 13.548 | 1.325 |
| find-transaction: peak RSS (MiB) | 7.22 | 11.98 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.76 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 36.50 | 82.68 | 48.19 |
| Write throughput (Mobs/s) | 36.745 | 16.223 | 27.834 |
| Read time (ms) | 16.35 | 37.84 | 40.54 |
| Read throughput (Mobs/s) | 82.061 | 35.448 | 33.085 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.70 | 41.11 | 6.10 |
| Read peak RSS (MiB) | 7.46 | 9.42 | 6.11 |
| Capture time (ms) | 32.21 | 45.29 | 47.51 |
| Capture time (ns/tick) | 1342.2 | 1887.1 | 1979.5 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 4.14 | 37.22 | 0.22 |
| Write CPU time (ms) | 37.12 | 83.33 | 48.93 |
| Write min / max (ms) | 36.48 / 37.11 | 81.53 / 84.20 | 46.46 / 49.25 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.084 | 0.217 | 11.778 |
| search-signal: p95 (ms) | 0.222 | 1.866 | 16.612 |
| search-signal: peak RSS (MiB) | 6.68 | 9.88 | 6.69 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.100 | 0.205 | 17.814 |
| find-signal: p95 (ms) | 0.237 | 1.677 | 18.359 |
| find-signal: peak RSS (MiB) | 6.72 | 9.87 | 6.66 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.086 | 0.035 | 11.916 |
| point: p95 (ms) | 0.215 | 0.135 | 16.687 |
| point: peak RSS (MiB) | 6.62 | 5.25 | 6.68 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.144 | 0.430 | 0.435 |
| search-transaction: p95 (ms) | 0.207 | 1.079 | 1.099 |
| search-transaction: peak RSS (MiB) | 5.92 | 6.15 | 6.14 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 3.786 | 1.038 | 1.069 |
| find-transaction: p95 (ms) | 4.573 | 1.107 | 1.135 |
| find-transaction: peak RSS (MiB) | 5.86 | 6.16 | 6.17 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
