# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **54.60 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 48.90 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 114.44 | 704.07 | 167.85 |
| Write throughput (Mobs/s) | 54.971 | 8.935 | 37.481 |
| Read time (ms) | 43.39 | 170.03 | 181.34 |
| Read throughput (Mobs/s) | 144.983 | 36.999 | 34.693 |
| File size (MiB) | 7.666 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.91 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 53.21 | 140.89 | 5.40 |
| Read peak RSS (MiB) | 17.69 | 26.07 | 5.18 |
| Capture time (ms) | 86.48 | 582.64 | 167.22 |
| Capture time (ns/tick) | 180.2 | 1213.8 | 348.4 |
| File bytes per tick | 16.7 | 19.5 | 326.2 |
| Close time (ms) | 27.77 | 121.34 | 0.13 |
| Write CPU time (ms) | 115.48 | 705.06 | 168.85 |
| Write min / max (ms) | 112.90 / 114.87 | 703.01 / 704.14 | 167.63 / 168.87 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.466 | 2.529 | 56.141 |
| search-signal: p95 (ms) | 1.960 | 6.271 | 89.383 |
| search-signal: peak RSS (MiB) | 12.69 | 26.90 | 6.60 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.702 | 3.184 | 93.542 |
| find-signal: p95 (ms) | 6.032 | 8.660 | 97.096 |
| find-signal: peak RSS (MiB) | 12.62 | 26.91 | 6.57 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.535 | 0.360 | 56.068 |
| point: p95 (ms) | 2.384 | 1.557 | 90.109 |
| point: peak RSS (MiB) | 12.63 | 8.83 | 6.55 |

### soc

Observations: 20,143,793. Null producer: 143.23 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 387.46 | 851.71 | 515.77 |
| Write throughput (Mobs/s) | 51.990 | 23.651 | 39.056 |
| Read time (ms) | 111.09 | 598.18 | 574.31 |
| Read throughput (Mobs/s) | 181.325 | 33.675 | 35.075 |
| File size (MiB) | 24.806 | 52.345 | 485.555 |
| Bytes per observation | 1.29 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.78 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 114.39 | 145.45 | 5.57 |
| Read peak RSS (MiB) | 30.14 | 22.54 | 5.21 |
| Capture time (ms) | 359.99 | 826.18 | 514.82 |
| Capture time (ns/tick) | 14999.5 | 34424.2 | 21450.8 |
| File bytes per tick | 1083.8 | 2287.0 | 21214.2 |
| Close time (ms) | 26.36 | 24.73 | 0.17 |
| Write CPU time (ms) | 388.49 | 852.60 | 516.75 |
| Write min / max (ms) | 386.69 / 397.64 | 846.53 / 881.28 | 514.81 / 516.28 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.290 | 0.452 | 186.996 |
| search-signal: p95 (ms) | 0.495 | 6.862 | 262.649 |
| search-signal: peak RSS (MiB) | 6.40 | 22.58 | 6.57 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.324 | 7.390 | 283.152 |
| find-signal: p95 (ms) | 0.575 | 7.658 | 288.666 |
| find-signal: peak RSS (MiB) | 6.59 | 22.32 | 6.59 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.302 | 0.200 | 184.517 |
| point: p95 (ms) | 0.484 | 0.318 | 262.026 |
| point: peak RSS (MiB) | 6.40 | 4.92 | 6.51 |

### idle

Observations: 2,547,072. Null producer: 18.28 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 59.70 | 117.46 | 66.63 |
| Write throughput (Mobs/s) | 42.662 | 21.684 | 38.230 |
| Read time (ms) | 16.06 | 74.52 | 72.80 |
| Read throughput (Mobs/s) | 158.644 | 34.179 | 34.986 |
| File size (MiB) | 1.853 | 4.165 | 61.551 |
| Bytes per observation | 0.76 | 1.71 | 25.34 |
| Logical bytes / file bytes | 19.95 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 22.14 | 77.52 | 5.58 |
| Read peak RSS (MiB) | 7.20 | 14.63 | 5.10 |
| Capture time (ms) | 36.75 | 59.90 | 65.73 |
| Capture time (ns/tick) | 765.6 | 1247.8 | 1369.4 |
| File bytes per tick | 40.5 | 91.0 | 1344.6 |
| Close time (ms) | 21.74 | 56.94 | 0.17 |
| Write CPU time (ms) | 60.60 | 118.37 | 67.51 |
| Write min / max (ms) | 59.01 / 59.94 | 116.91 / 117.60 | 66.56 / 66.69 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.340 | 0.335 | 13.711 |
| search-signal: p95 (ms) | 0.544 | 4.321 | 33.332 |
| search-signal: peak RSS (MiB) | 5.86 | 14.98 | 6.54 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.339 | 0.335 | 35.410 |
| find-signal: p95 (ms) | 0.545 | 4.268 | 36.399 |
| find-signal: peak RSS (MiB) | 5.89 | 15.00 | 6.56 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.334 | 0.096 | 13.338 |
| point: p95 (ms) | 0.521 | 0.321 | 33.469 |
| point: peak RSS (MiB) | 5.89 | 5.38 | 6.54 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.62 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 30.11 | 37.91 |
| Write throughput (Mobs/s) | 6.832 | 5.427 |
| Read time (ms) | 16.57 | 11.54 |
| Read throughput (Mobs/s) | 12.414 | 17.820 |
| File size (MiB) | 2.292 | 4.424 |
| Bytes per observation | 11.68 | 22.55 |
| Logical bytes / file bytes | 5.82 | 3.02 |
| Write peak RSS (MiB) | 9.65 | 4.88 |
| Read peak RSS (MiB) | 7.70 | 8.62 |
| Capture time (ms) | 29.02 | 37.68 |
| Capture time (ns/tick) | 241.8 | 314.0 |
| File bytes per tick | 20.0 | 38.7 |
| Close time (ms) | 1.03 | 0.18 |
| Write CPU time (ms) | 31.06 | 38.78 |
| Write min / max (ms) | 29.90 / 30.72 | 37.71 / 38.75 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 1.932 | 3.497 |
| search-transaction: p95 (ms) | 2.392 | 4.379 |
| search-transaction: peak RSS (MiB) | 6.10 | 8.64 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 10.631 | 5.017 |
| find-transaction: p95 (ms) | 11.845 | 5.220 |
| find-transaction: peak RSS (MiB) | 7.72 | 8.61 |

### many-generators

Observations: 329,168. Null producer: 13.89 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 55.42 | 67.57 |
| Write throughput (Mobs/s) | 5.940 | 4.871 |
| Read time (ms) | 29.17 | 21.15 |
| Read throughput (Mobs/s) | 11.285 | 15.563 |
| File size (MiB) | 3.775 | 7.571 |
| Bytes per observation | 12.02 | 24.12 |
| Logical bytes / file bytes | 5.66 | 2.82 |
| Write peak RSS (MiB) | 9.25 | 6.05 |
| Read peak RSS (MiB) | 9.25 | 11.84 |
| Capture time (ms) | 54.00 | 66.83 |
| Capture time (ns/tick) | 2250.0 | 2784.5 |
| File bytes per tick | 164.9 | 330.8 |
| Close time (ms) | 1.31 | 0.70 |
| Write CPU time (ms) | 56.20 | 68.51 |
| Write min / max (ms) | 55.04 / 56.52 | 66.97 / 67.73 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 1.911 | 0.626 |
| search-transaction: p95 (ms) | 3.772 | 1.387 |
| search-transaction: peak RSS (MiB) | 6.80 | 11.90 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 16.703 | 1.359 |
| find-transaction: p95 (ms) | 17.866 | 1.506 |
| find-transaction: peak RSS (MiB) | 9.17 | 11.86 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 12.07 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 37.01 | 98.32 | 49.69 |
| Write throughput (Mobs/s) | 36.239 | 13.642 | 26.990 |
| Read time (ms) | 15.24 | 38.73 | 41.48 |
| Read throughput (Mobs/s) | 88.036 | 34.631 | 32.334 |
| File size (MiB) | 2.500 | 4.010 | 31.812 |
| Bytes per observation | 1.95 | 3.13 | 24.87 |
| Logical bytes / file bytes | 9.44 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 17.34 | 40.95 | 6.02 |
| Read peak RSS (MiB) | 8.17 | 9.32 | 6.07 |
| Capture time (ms) | 31.02 | 46.22 | 48.91 |
| Capture time (ns/tick) | 1292.7 | 1926.0 | 2038.1 |
| File bytes per tick | 109.2 | 175.2 | 1389.9 |
| Close time (ms) | 5.83 | 51.90 | 0.27 |
| Write CPU time (ms) | 38.02 | 99.19 | 50.66 |
| Write min / max (ms) | 37.00 / 37.15 | 95.68 / 102.79 | 49.58 / 50.09 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.057 | 0.237 | 12.533 |
| search-signal: p95 (ms) | 0.204 | 2.097 | 16.990 |
| search-signal: peak RSS (MiB) | 6.05 | 9.79 | 6.57 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.070 | 0.243 | 18.428 |
| find-signal: p95 (ms) | 0.338 | 2.062 | 19.209 |
| find-signal: peak RSS (MiB) | 6.04 | 9.74 | 6.55 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.062 | 0.038 | 12.333 |
| point: p95 (ms) | 0.216 | 0.171 | 17.176 |
| point: peak RSS (MiB) | 6.03 | 5.11 | 6.57 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 2.076 | 0.443 | 0.431 |
| search-transaction: p95 (ms) | 2.775 | 1.110 | 1.094 |
| search-transaction: peak RSS (MiB) | 6.54 | 6.00 | 6.07 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 4.499 | 1.086 | 1.093 |
| find-transaction: p95 (ms) | 5.679 | 1.203 | 1.173 |
| find-transaction: peak RSS (MiB) | 6.55 | 6.04 | 6.04 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. VTR uses its default reader mode without whole-section CRC scans; validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
