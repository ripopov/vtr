# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **54.74 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.0-38-generic-x86_64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 48.98 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 112.69 | 705.07 | 167.19 |
| Write throughput (Mobs/s) | 55.824 | 8.923 | 37.629 |
| Read time (ms) | 42.13 | 169.28 | 184.04 |
| Read throughput (Mobs/s) | 149.321 | 37.163 | 34.183 |
| File size (MiB) | 7.666 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.91 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 51.68 | 140.91 | 5.44 |
| Read peak RSS (MiB) | 17.75 | 26.23 | 5.08 |
| Capture time (ms) | 87.69 | 581.05 | 166.56 |
| Capture time (ns/tick) | 182.7 | 1210.5 | 347.0 |
| File bytes per tick | 16.7 | 19.5 | 326.2 |
| Close time (ms) | 24.88 | 123.32 | 0.12 |
| Write CPU time (ms) | 113.75 | 706.07 | 168.06 |
| Write min / max (ms) | 112.27 / 112.85 | 704.27 / 710.13 | 167.00 / 168.09 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.451 | 2.524 | 57.411 |
| search-signal: p95 (ms) | 1.892 | 6.249 | 91.174 |
| search-signal: peak RSS (MiB) | 12.71 | 26.90 | 6.54 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 0.698 | 3.183 | 97.064 |
| find-signal: p95 (ms) | 6.211 | 8.945 | 100.537 |
| find-signal: peak RSS (MiB) | 12.73 | 26.84 | 6.55 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.541 | 0.361 | 57.161 |
| point: p95 (ms) | 2.292 | 1.598 | 91.208 |
| point: peak RSS (MiB) | 12.69 | 8.87 | 6.55 |

### soc

Observations: 20,143,793. Null producer: 143.09 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 364.10 | 848.41 | 514.14 |
| Write throughput (Mobs/s) | 55.324 | 23.743 | 39.179 |
| Read time (ms) | 109.24 | 594.02 | 572.86 |
| Read throughput (Mobs/s) | 184.407 | 33.911 | 35.163 |
| File size (MiB) | 24.806 | 52.345 | 485.555 |
| Bytes per observation | 1.29 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.78 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 114.34 | 145.43 | 5.59 |
| Read peak RSS (MiB) | 30.18 | 22.57 | 5.18 |
| Capture time (ms) | 346.95 | 823.62 | 512.96 |
| Capture time (ns/tick) | 14456.2 | 34317.4 | 21373.4 |
| File bytes per tick | 1083.8 | 2287.0 | 21214.2 |
| Close time (ms) | 15.66 | 24.26 | 0.19 |
| Write CPU time (ms) | 365.03 | 849.35 | 514.83 |
| Write min / max (ms) | 363.73 / 369.90 | 847.95 / 868.92 | 513.73 / 515.47 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 0.282 | 0.444 | 186.304 |
| search-signal: p95 (ms) | 0.477 | 6.843 | 264.164 |
| search-signal: peak RSS (MiB) | 6.32 | 22.55 | 6.56 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 0.319 | 7.393 | 282.082 |
| find-signal: p95 (ms) | 0.580 | 7.953 | 287.230 |
| find-signal: peak RSS (MiB) | 6.68 | 22.31 | 6.57 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.292 | 0.199 | 185.376 |
| point: p95 (ms) | 0.483 | 0.319 | 261.043 |
| point: peak RSS (MiB) | 6.39 | 4.95 | 6.54 |

### idle

Observations: 2,547,072. Null producer: 18.13 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 50.43 | 117.76 | 66.80 |
| Write throughput (Mobs/s) | 50.504 | 21.629 | 38.130 |
| Read time (ms) | 15.32 | 74.93 | 73.79 |
| Read throughput (Mobs/s) | 166.260 | 33.995 | 34.519 |
| File size (MiB) | 1.853 | 4.165 | 61.551 |
| Bytes per observation | 0.76 | 1.71 | 25.34 |
| Logical bytes / file bytes | 19.95 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 22.16 | 77.52 | 5.59 |
| Read peak RSS (MiB) | 7.23 | 14.60 | 5.20 |
| Capture time (ms) | 36.52 | 60.01 | 65.87 |
| Capture time (ns/tick) | 760.8 | 1250.3 | 1372.3 |
| File bytes per tick | 40.5 | 91.0 | 1344.6 |
| Close time (ms) | 12.77 | 57.15 | 0.17 |
| Write CPU time (ms) | 51.40 | 118.55 | 67.71 |
| Write min / max (ms) | 50.28 / 50.91 | 116.99 / 117.99 | 66.79 / 66.89 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.336 | 0.339 | 14.439 |
| search-signal: p95 (ms) | 0.531 | 4.294 | 35.250 |
| search-signal: peak RSS (MiB) | 5.82 | 15.02 | 6.52 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.333 | 0.342 | 36.376 |
| find-signal: p95 (ms) | 0.531 | 4.351 | 37.124 |
| find-signal: peak RSS (MiB) | 5.89 | 14.95 | 6.55 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.330 | 0.095 | 14.214 |
| point: p95 (ms) | 0.522 | 0.325 | 35.456 |
| point: peak RSS (MiB) | 5.89 | 5.37 | 6.55 |

## Transaction results

### requests

Observations: 205,728. Null producer: 7.22 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 30.39 | 38.22 |
| Write throughput (Mobs/s) | 6.769 | 5.382 |
| Read time (ms) | 16.64 | 11.54 |
| Read throughput (Mobs/s) | 12.365 | 17.823 |
| File size (MiB) | 2.292 | 4.424 |
| Bytes per observation | 11.68 | 22.55 |
| Logical bytes / file bytes | 5.82 | 3.02 |
| Write peak RSS (MiB) | 9.66 | 4.80 |
| Read peak RSS (MiB) | 7.75 | 8.68 |
| Capture time (ms) | 29.13 | 37.99 |
| Capture time (ns/tick) | 242.8 | 316.6 |
| File bytes per tick | 20.0 | 38.7 |
| Close time (ms) | 1.06 | 0.17 |
| Write CPU time (ms) | 31.50 | 39.11 |
| Write min / max (ms) | 30.22 / 31.27 | 37.95 / 38.23 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 1.932 | 3.628 |
| search-transaction: p95 (ms) | 2.452 | 4.384 |
| search-transaction: peak RSS (MiB) | 6.18 | 8.60 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 10.671 | 5.163 |
| find-transaction: p95 (ms) | 11.891 | 5.451 |
| find-transaction: peak RSS (MiB) | 7.67 | 8.71 |

### many-generators

Observations: 329,168. Null producer: 13.95 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 55.85 | 67.91 |
| Write throughput (Mobs/s) | 5.894 | 4.847 |
| Read time (ms) | 29.33 | 21.16 |
| Read throughput (Mobs/s) | 11.223 | 15.557 |
| File size (MiB) | 3.775 | 7.571 |
| Bytes per observation | 12.02 | 24.12 |
| Logical bytes / file bytes | 5.66 | 2.82 |
| Write peak RSS (MiB) | 9.15 | 6.03 |
| Read peak RSS (MiB) | 9.21 | 11.84 |
| Capture time (ms) | 54.43 | 67.15 |
| Capture time (ns/tick) | 2267.9 | 2798.0 |
| File bytes per tick | 164.9 | 330.8 |
| Close time (ms) | 1.30 | 0.70 |
| Write CPU time (ms) | 56.76 | 68.83 |
| Write min / max (ms) | 55.65 / 55.95 | 67.12 / 68.50 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 1.931 | 0.621 |
| search-transaction: p95 (ms) | 3.854 | 1.416 |
| search-transaction: peak RSS (MiB) | 6.81 | 11.84 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 16.868 | 1.347 |
| find-transaction: p95 (ms) | 18.070 | 1.496 |
| find-transaction: peak RSS (MiB) | 9.27 | 11.88 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 12.11 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 36.39 | 96.43 | 49.80 |
| Write throughput (Mobs/s) | 36.861 | 13.909 | 26.931 |
| Read time (ms) | 14.86 | 38.70 | 42.17 |
| Read throughput (Mobs/s) | 90.231 | 34.660 | 31.808 |
| File size (MiB) | 2.500 | 4.010 | 31.812 |
| Bytes per observation | 1.95 | 3.13 | 24.87 |
| Logical bytes / file bytes | 9.44 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 17.26 | 40.98 | 5.94 |
| Read peak RSS (MiB) | 8.18 | 9.20 | 5.98 |
| Capture time (ms) | 31.56 | 46.56 | 49.03 |
| Capture time (ns/tick) | 1314.9 | 1939.9 | 2042.9 |
| File bytes per tick | 109.2 | 175.2 | 1389.9 |
| Close time (ms) | 4.66 | 49.44 | 0.27 |
| Write CPU time (ms) | 37.33 | 97.48 | 50.74 |
| Write min / max (ms) | 35.96 / 36.46 | 95.35 / 97.59 | 49.60 / 50.65 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.053 | 0.239 | 12.189 |
| search-signal: p95 (ms) | 0.195 | 2.062 | 17.156 |
| search-signal: peak RSS (MiB) | 6.03 | 9.71 | 6.56 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.070 | 0.231 | 18.615 |
| find-signal: p95 (ms) | 0.341 | 2.003 | 19.321 |
| find-signal: peak RSS (MiB) | 5.98 | 9.76 | 6.55 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.058 | 0.037 | 12.261 |
| point: p95 (ms) | 0.197 | 0.163 | 17.365 |
| point: peak RSS (MiB) | 6.05 | 5.13 | 6.52 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 2.091 | 0.446 | 0.447 |
| search-transaction: p95 (ms) | 2.756 | 1.094 | 1.093 |
| search-transaction: peak RSS (MiB) | 6.57 | 6.00 | 6.01 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 4.528 | 1.104 | 1.091 |
| find-transaction: p95 (ms) | 5.767 | 1.283 | 1.180 |
| find-transaction: peak RSS (MiB) | 6.56 | 6.08 | 6.04 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. VTR uses its default reader mode without whole-section CRC scans; validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.
