# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **52.79 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 48.12 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 109.79 | 615.04 | 166.06 |
| Write throughput (Mobs/s) | 57.302 | 10.229 | 37.885 |
| Read time (ms) | 39.04 | 167.93 | 183.93 |
| Read throughput (Mobs/s) | 161.127 | 37.463 | 34.203 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 18.84 | 141.05 | 5.58 |
| Read peak RSS (MiB) | 14.08 | 26.29 | 5.35 |
| Capture time (ms) | 108.32 | 533.76 | 165.45 |
| Capture time (ns/tick) | 225.7 | 1112.0 | 344.7 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.36 | 81.14 | 0.11 |
| Write CPU time (ms) | 110.69 | 615.66 | 166.78 |
| Write min / max (ms) | 108.17 / 113.17 | 589.50 / 687.86 | 162.34 / 167.10 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.112 | 2.494 | 56.928 |
| search-signal: p95 (ms) | 0.422 | 6.186 | 90.369 |
| search-signal: peak RSS (MiB) | 7.80 | 26.93 | 6.67 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.627 | 2.932 | 95.154 |
| find-signal: p95 (ms) | 2.669 | 16.193 | 99.213 |
| find-signal: peak RSS (MiB) | 10.33 | 26.99 | 6.69 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.120 | 0.361 | 56.057 |
| point: p95 (ms) | 0.439 | 1.332 | 89.494 |
| point: peak RSS (MiB) | 7.66 | 8.98 | 6.59 |

### soc

Observations: 20,143,793. Null producer: 138.56 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 418.82 | 826.80 | 497.22 |
| Write throughput (Mobs/s) | 48.096 | 24.363 | 40.513 |
| Read time (ms) | 120.67 | 591.31 | 560.27 |
| Read throughput (Mobs/s) | 166.932 | 34.067 | 35.954 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 16.21 | 145.61 | 5.71 |
| Read peak RSS (MiB) | 30.77 | 22.66 | 5.27 |
| Capture time (ms) | 410.84 | 802.08 | 496.23 |
| Capture time (ns/tick) | 17118.1 | 33420.2 | 20676.1 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 6.54 | 24.13 | 0.17 |
| Write CPU time (ms) | 419.52 | 827.48 | 497.76 |
| Write min / max (ms) | 401.62 / 419.25 | 820.60 / 838.64 | 496.78 / 498.97 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.430 | 0.416 | 181.303 |
| search-signal: p95 (ms) | 0.689 | 6.573 | 254.310 |
| search-signal: peak RSS (MiB) | 7.12 | 22.72 | 6.68 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.572 | 7.028 | 272.007 |
| find-signal: p95 (ms) | 0.767 | 7.577 | 278.434 |
| find-signal: peak RSS (MiB) | 9.42 | 22.43 | 6.71 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.426 | 0.195 | 179.242 |
| point: p95 (ms) | 0.657 | 0.292 | 255.252 |
| point: peak RSS (MiB) | 7.08 | 5.11 | 6.64 |

### idle

Observations: 2,547,072. Null producer: 17.58 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 53.72 | 113.31 | 62.87 |
| Write throughput (Mobs/s) | 47.411 | 22.479 | 40.516 |
| Read time (ms) | 16.59 | 73.01 | 71.33 |
| Read throughput (Mobs/s) | 153.563 | 34.886 | 35.709 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 15.48 | 77.65 | 5.72 |
| Read peak RSS (MiB) | 8.04 | 14.74 | 5.24 |
| Capture time (ms) | 45.85 | 58.31 | 61.98 |
| Capture time (ns/tick) | 955.2 | 1214.9 | 1291.2 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 6.73 | 54.43 | 0.16 |
| Write CPU time (ms) | 54.40 | 113.92 | 63.48 |
| Write min / max (ms) | 53.67 / 56.48 | 110.22 / 114.22 | 62.76 / 65.40 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.383 | 0.329 | 13.620 |
| search-signal: p95 (ms) | 0.643 | 3.887 | 34.029 |
| search-signal: peak RSS (MiB) | 6.80 | 15.11 | 6.68 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.418 | 0.326 | 34.677 |
| find-signal: p95 (ms) | 0.671 | 3.644 | 35.835 |
| find-signal: peak RSS (MiB) | 6.82 | 15.11 | 6.66 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.393 | 0.095 | 13.379 |
| point: p95 (ms) | 0.589 | 0.276 | 33.627 |
| point: peak RSS (MiB) | 6.68 | 5.52 | 6.69 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.08 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 28.28 | 38.37 |
| Write throughput (Mobs/s) | 7.275 | 5.362 |
| Read time (ms) | 21.18 | 10.93 |
| Read throughput (Mobs/s) | 9.714 | 18.815 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 6.45 | 5.02 |
| Read peak RSS (MiB) | 6.37 | 8.80 |
| Capture time (ms) | 27.98 | 38.19 |
| Capture time (ns/tick) | 233.2 | 318.2 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.12 | 0.16 |
| Write CPU time (ms) | 29.10 | 38.92 |
| Write min / max (ms) | 27.56 / 28.59 | 36.15 / 38.57 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.112 | 3.344 |
| search-transaction: p95 (ms) | 0.171 | 4.190 |
| search-transaction: peak RSS (MiB) | 6.38 | 8.82 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 8.969 | 4.745 |
| find-transaction: p95 (ms) | 10.823 | 4.949 |
| find-transaction: peak RSS (MiB) | 6.19 | 8.81 |

### many-generators

Observations: 329,168. Null producer: 13.74 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 51.31 | 65.87 |
| Write throughput (Mobs/s) | 6.415 | 4.997 |
| Read time (ms) | 36.16 | 20.24 |
| Read throughput (Mobs/s) | 9.102 | 16.261 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 6.59 | 6.26 |
| Read peak RSS (MiB) | 7.07 | 11.83 |
| Capture time (ms) | 50.98 | 65.12 |
| Capture time (ns/tick) | 2124.1 | 2713.5 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.14 | 0.65 |
| Write CPU time (ms) | 52.12 | 66.57 |
| Write min / max (ms) | 51.25 / 53.42 | 65.46 / 66.22 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.291 | 0.610 |
| search-transaction: p95 (ms) | 0.386 | 1.348 |
| search-transaction: peak RSS (MiB) | 7.05 | 12.01 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 12.386 | 1.288 |
| find-transaction: p95 (ms) | 15.074 | 1.363 |
| find-transaction: peak RSS (MiB) | 7.18 | 11.98 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.95 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 36.58 | 83.26 | 49.49 |
| Write throughput (Mobs/s) | 36.669 | 16.111 | 27.104 |
| Read time (ms) | 16.43 | 38.48 | 41.13 |
| Read throughput (Mobs/s) | 81.623 | 34.854 | 32.608 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 15.79 | 41.06 | 6.30 |
| Read peak RSS (MiB) | 7.52 | 9.39 | 6.12 |
| Capture time (ms) | 32.05 | 45.66 | 48.76 |
| Capture time (ns/tick) | 1335.5 | 1902.6 | 2031.7 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 4.28 | 36.75 | 0.26 |
| Write CPU time (ms) | 37.39 | 84.00 | 50.24 |
| Write min / max (ms) | 36.08 / 37.40 | 82.26 / 84.84 | 47.96 / 49.66 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.083 | 0.220 | 11.605 |
| search-signal: p95 (ms) | 0.247 | 1.920 | 16.875 |
| search-signal: peak RSS (MiB) | 6.74 | 9.89 | 6.70 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.100 | 0.204 | 17.937 |
| find-signal: p95 (ms) | 0.248 | 1.790 | 18.426 |
| find-signal: peak RSS (MiB) | 6.66 | 9.88 | 6.62 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.086 | 0.036 | 11.910 |
| point: p95 (ms) | 0.222 | 0.135 | 16.843 |
| point: peak RSS (MiB) | 6.71 | 5.26 | 6.68 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.144 | 0.444 | 0.442 |
| search-transaction: p95 (ms) | 0.209 | 1.103 | 1.062 |
| search-transaction: peak RSS (MiB) | 5.79 | 6.17 | 6.17 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 3.614 | 1.076 | 1.088 |
| find-transaction: p95 (ms) | 4.718 | 1.131 | 1.143 |
| find-transaction: peak RSS (MiB) | 5.97 | 6.17 | 6.12 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
