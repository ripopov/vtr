# VTR synthetic format comparison

Status: **complete; all checks passed**. Elapsed suite time: **63.85 s** (budget 280 s; dependency setup/build excluded).

Host: Linux-7.0.14-linuxkit-aarch64-with-glibc2.43. CPU affinity: 0. Repeats: 3. Scale divisor: 1. Format-order seed: 20261018.

All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. Buffered I/O is used; fsync and storage durability are not measured. Reads decode every observation and checksum all values/attributes. Read/query results are checked against an independently replayed producer. Fresh processes isolate peak RSS; OS page cache is warm after writing. Every query opens a fresh reader, including metadata/index preparation and close in its latency.

Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. File size includes metadata and indexes (both files for paired formats). Logical bytes count fixed binary observation fields, not in-memory object sizes; logical/file is a defined normalization, not a claim about compression of identical input bytes. A dash indicates an unavailable measurement in an incomplete run.

Capture is the entire producer loop including synchronous tracer work and any stalls; it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. The null producer follows the same generation and lifetime schedule with a counting sink. CPU time includes process startup; wall timings start inside the executable.

Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. The find-signal and find-transaction operations apply the same predicates across the entire trace. Point queries return the value already in effect, including during idle windows. p95 is the nearest-rank percentile over all probes, not a confidence interval. All individual timings, counts, checksums, and build provenance are in raw.json.

## Waveform results

### core

Observations: 6,291,079. Null producer: 42.92 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 102.79 | 455.30 | 198.50 |
| Write throughput (Mobs/s) | 61.203 | 13.818 | 31.693 |
| Read time (ms) | 46.28 | 139.77 | 219.53 |
| Read throughput (Mobs/s) | 135.939 | 45.010 | 28.657 |
| File size (MiB) | 7.695 | 8.941 | 149.324 |
| Bytes per observation | 1.28 | 1.49 | 24.89 |
| Logical bytes / file bytes | 11.86 | 10.21 | 0.61 |
| Write peak RSS (MiB) | 21.45 | 141.25 | 4.64 |
| Read peak RSS (MiB) | 12.84 | 25.67 | 4.59 |
| Capture time (ms) | 100.74 | 396.32 | 197.81 |
| Capture time (ns/tick) | 209.9 | 825.7 | 412.1 |
| File bytes per tick | 16.8 | 19.5 | 326.2 |
| Close time (ms) | 1.83 | 58.49 | 0.34 |
| Write CPU time (ms) | 100.57 | 448.57 | 163.67 |
| Write min / max (ms) | 98.30 / 107.65 | 437.44 / 505.33 | 197.45 / 224.44 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 291 | 291 | 291 |
| search-signal: p50 (ms) | 0.571 | 1.387 | 81.336 |
| search-signal: p95 (ms) | 0.859 | 5.090 | 126.951 |
| search-signal: peak RSS (MiB) | 7.16 | 27.54 | 6.89 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 1,068,627 | 1,068,627 | 1,068,627 |
| find-signal: p50 (ms) | 1.774 | 1.774 | 128.227 |
| find-signal: p95 (ms) | 4.269 | 6.685 | 136.660 |
| find-signal: peak RSS (MiB) | 7.84 | 27.82 | 7.13 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.550 | 0.528 | 79.185 |
| point: p95 (ms) | 0.845 | 0.972 | 123.060 |
| point: peak RSS (MiB) | 5.73 | 9.31 | 7.08 |

### soc

Observations: 20,143,793. Null producer: 114.40 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 404.59 | 895.31 | 571.61 |
| Write throughput (Mobs/s) | 49.788 | 22.499 | 35.241 |
| Read time (ms) | 143.57 | 471.27 | 637.30 |
| Read throughput (Mobs/s) | 140.309 | 42.744 | 31.608 |
| File size (MiB) | 24.579 | 52.345 | 485.555 |
| Bytes per observation | 1.28 | 2.72 | 25.28 |
| Logical bytes / file bytes | 11.89 | 5.58 | 0.60 |
| Write peak RSS (MiB) | 17.79 | 147.62 | 5.76 |
| Read peak RSS (MiB) | 29.87 | 23.59 | 4.59 |
| Capture time (ms) | 396.30 | 858.74 | 570.74 |
| Capture time (ns/tick) | 16512.5 | 35780.9 | 23781.0 |
| File bytes per tick | 1073.9 | 2287.0 | 21214.2 |
| Close time (ms) | 7.32 | 34.64 | 0.36 |
| Write CPU time (ms) | 393.76 | 730.44 | 460.49 |
| Write min / max (ms) | 389.49 / 413.12 | 893.60 / 939.03 | 565.65 / 580.51 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 297 | 297 | 297 |
| search-signal: p50 (ms) | 1.007 | 0.559 | 247.532 |
| search-signal: p95 (ms) | 3.480 | 1.341 | 345.316 |
| search-signal: peak RSS (MiB) | 7.93 | 23.57 | 7.24 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,517 | 53,517 | 53,517 |
| find-signal: p50 (ms) | 2.083 | 1.128 | 371.999 |
| find-signal: p95 (ms) | 2.695 | 1.497 | 415.718 |
| find-signal: peak RSS (MiB) | 9.96 | 26.75 | 7.18 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.966 | 0.419 | 242.478 |
| point: p95 (ms) | 1.309 | 0.536 | 342.329 |
| point: peak RSS (MiB) | 7.37 | 5.91 | 7.05 |

### idle

Observations: 2,547,072. Null producer: 15.64 ms.

| Metric | VTR | FST | VCD |
|---|---:|---:|---:|
| Write time (ms) | 54.99 | 102.31 | 76.81 |
| Write throughput (Mobs/s) | 46.320 | 24.895 | 33.160 |
| Read time (ms) | 18.86 | 59.21 | 81.89 |
| Read throughput (Mobs/s) | 135.045 | 43.015 | 31.104 |
| File size (MiB) | 1.732 | 4.165 | 61.551 |
| Bytes per observation | 0.71 | 1.71 | 25.34 |
| Logical bytes / file bytes | 21.34 | 8.88 | 0.60 |
| Write peak RSS (MiB) | 16.91 | 78.73 | 4.66 |
| Read peak RSS (MiB) | 8.73 | 15.73 | 4.59 |
| Capture time (ms) | 46.22 | 30.48 | 75.86 |
| Capture time (ns/tick) | 962.9 | 635.1 | 1580.3 |
| File bytes per tick | 37.8 | 91.0 | 1344.6 |
| Close time (ms) | 7.89 | 70.90 | 0.48 |
| Write CPU time (ms) | 53.67 | 83.74 | 62.18 |
| Write min / max (ms) | 54.36 / 69.93 | 101.44 / 116.98 | 76.32 / 80.44 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 3 | 3 | 3 |
| search-signal: p50 (ms) | 0.505 | 0.325 | 19.384 |
| search-signal: p95 (ms) | 0.777 | 0.472 | 46.662 |
| search-signal: peak RSS (MiB) | 7.40 | 15.47 | 7.27 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 6,744 | 6,744 | 6,744 |
| find-signal: p50 (ms) | 0.626 | 0.309 | 48.059 |
| find-signal: p95 (ms) | 0.821 | 0.389 | 48.653 |
| find-signal: peak RSS (MiB) | 7.68 | 15.82 | 6.51 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.503 | 0.261 | 18.629 |
| point: p95 (ms) | 0.641 | 0.380 | 45.912 |
| point: peak RSS (MiB) | 6.77 | 5.87 | 6.96 |

## Transaction results

### requests

Observations: 205,728. Null producer: 8.07 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 33.58 | 38.15 |
| Write throughput (Mobs/s) | 6.127 | 5.393 |
| Read time (ms) | 16.42 | 13.45 |
| Read throughput (Mobs/s) | 12.526 | 15.294 |
| File size (MiB) | 1.046 | 4.424 |
| Bytes per observation | 5.33 | 22.55 |
| Logical bytes / file bytes | 12.75 | 3.02 |
| Write peak RSS (MiB) | 5.96 | 5.73 |
| Read peak RSS (MiB) | 6.99 | 7.57 |
| Capture time (ms) | 32.91 | 37.57 |
| Capture time (ns/tick) | 274.2 | 313.1 |
| File bytes per tick | 9.1 | 38.7 |
| Close time (ms) | 0.34 | 0.40 |
| Write CPU time (ms) | 29.37 | 34.80 |
| Write min / max (ms) | 33.34 / 34.37 | 36.97 / 38.40 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 27 | 27 |
| search-transaction: p50 (ms) | 0.600 | 5.145 |
| search-transaction: p95 (ms) | 0.747 | 6.294 |
| search-transaction: peak RSS (MiB) | 5.07 | 9.55 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 2,715 | 2,715 |
| find-transaction: p50 (ms) | 4.936 | 6.522 |
| find-transaction: p95 (ms) | 5.477 | 8.006 |
| find-transaction: peak RSS (MiB) | 5.10 | 7.57 |

### many-generators

Observations: 329,168. Null producer: 16.22 ms.

| Metric | VTR | FTR |
|---|---:|---:|
| Write time (ms) | 59.17 | 70.50 |
| Write throughput (Mobs/s) | 5.563 | 4.669 |
| Read time (ms) | 28.00 | 25.00 |
| Read throughput (Mobs/s) | 11.755 | 13.168 |
| File size (MiB) | 1.662 | 7.571 |
| Bytes per observation | 5.29 | 24.12 |
| Logical bytes / file bytes | 12.84 | 2.82 |
| Write peak RSS (MiB) | 7.04 | 7.07 |
| Read peak RSS (MiB) | 7.63 | 10.78 |
| Capture time (ms) | 58.50 | 65.54 |
| Capture time (ns/tick) | 2437.7 | 2731.0 |
| File bytes per tick | 72.6 | 330.8 |
| Close time (ms) | 0.31 | 3.74 |
| Write CPU time (ms) | 52.40 | 61.48 |
| Write min / max (ms) | 57.59 / 60.33 | 69.19 / 72.30 |
| search-transaction: probes | 36 | 36 |
| search-transaction: matches | 15 | 15 |
| search-transaction: p50 (ms) | 0.966 | 3.378 |
| search-transaction: p95 (ms) | 1.045 | 4.227 |
| search-transaction: peak RSS (MiB) | 7.81 | 10.40 |
| find-transaction: probes | 36 | 36 |
| find-transaction: matches | 537 | 537 |
| find-transaction: p50 (ms) | 6.175 | 3.969 |
| find-transaction: p95 (ms) | 6.374 | 4.531 |
| find-transaction: peak RSS (MiB) | 5.90 | 10.40 |

## Mixed signal and transaction results

### mixed

Observations: 1,341,285. Null producer: 11.72 ms.

| Metric | VTR | FST+FTR | VCD+FTR |
|---|---:|---:|---:|
| Write time (ms) | 36.31 | 62.01 | 58.23 |
| Write throughput (Mobs/s) | 36.936 | 21.629 | 23.035 |
| Read time (ms) | 15.88 | 33.80 | 50.21 |
| Read throughput (Mobs/s) | 84.450 | 39.685 | 26.713 |
| File size (MiB) | 1.970 | 4.010 | 31.812 |
| Bytes per observation | 1.54 | 3.13 | 24.87 |
| Logical bytes / file bytes | 11.98 | 5.89 | 0.74 |
| Write peak RSS (MiB) | 16.59 | 42.64 | 7.10 |
| Read peak RSS (MiB) | 8.23 | 8.53 | 5.94 |
| Capture time (ms) | 31.43 | 30.92 | 57.22 |
| Capture time (ns/tick) | 1309.4 | 1288.5 | 2384.3 |
| File bytes per tick | 86.1 | 175.2 | 1389.9 |
| Close time (ms) | 3.36 | 30.39 | 0.88 |
| Write CPU time (ms) | 32.78 | 56.93 | 46.17 |
| Write min / max (ms) | 34.69 / 37.59 | 61.90 / 62.28 | 55.76 / 60.12 |
| search-signal: probes | 36 | 36 | 36 |
| search-signal: matches | 294 | 294 | 294 |
| search-signal: p50 (ms) | 0.421 | 0.249 | 18.445 |
| search-signal: p95 (ms) | 0.526 | 0.524 | 25.912 |
| search-signal: peak RSS (MiB) | 6.69 | 10.78 | 7.27 |
| find-signal: probes | 36 | 36 | 36 |
| find-signal: matches | 53,496 | 53,496 | 53,496 |
| find-signal: p50 (ms) | 0.411 | 0.247 | 27.653 |
| find-signal: p95 (ms) | 0.546 | 0.503 | 31.822 |
| find-signal: peak RSS (MiB) | 4.99 | 10.25 | 6.53 |
| point: probes | 36 | 36 | 36 |
| point: matches | 36 | 36 | 36 |
| point: p50 (ms) | 0.413 | 0.155 | 18.308 |
| point: p95 (ms) | 0.513 | 0.230 | 25.199 |
| point: peak RSS (MiB) | 6.63 | 4.11 | 6.87 |
| search-transaction: probes | 36 | 36 | 36 |
| search-transaction: matches | 24 | 24 | 24 |
| search-transaction: p50 (ms) | 0.339 | 0.965 | 0.959 |
| search-transaction: p95 (ms) | 0.401 | 1.594 | 1.619 |
| search-transaction: peak RSS (MiB) | 6.55 | 4.94 | 4.94 |
| find-transaction: probes | 36 | 36 | 36 |
| find-transaction: matches | 543 | 543 | 543 |
| find-transaction: p50 (ms) | 1.923 | 1.606 | 1.626 |
| find-transaction: p95 (ms) | 2.024 | 1.678 | 1.890 |
| find-transaction: peak RSS (MiB) | 6.70 | 4.94 | 4.94 |

## Interpretation

VTR uses the native C++20 reader and writer in vtr-cpp. FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. VCD uses the included buffered C++ writer and streaming subset reader. FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. These read timings compare the named implementations, not inherent format limits. Timed VTR reads and queries explicitly disable whole-section checksum verification; replay validation still checks every decoded observation against the producer. VTR and FST use native signal/time selection; VCD scans from the beginning. FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. The FTR header scan/index is charged on every open. No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, or rich owner/stage/relation navigation; those are outside these common-format workloads.

## Baseline comparison

All 523 numeric table entries were compared with the committed report and a
fresh baseline at `86ea3c9`, using the same Linux image, CPU 0, three repetitions,
full workload sizes, and format-order seed. A full repeat without overlapping
builds completed in 63.85 seconds, matching the fresh baseline's 63.85 seconds.
VTR write and read medians differ from that baseline by at most 4.4%; VTR file
sizes are identical across all six workloads. Capture, close, CPU time, memory,
and query measurements were also checked. No persistent performance regression
was identified.

Peak RSS occasionally varies by approximately 2 MiB between individual fresh
processes in both versions. For example, requests read RSS was 7160/5192/5184 KiB
in this run and 5196/5192/5260 KiB in the baseline: the maximum changes while the
median remains stable. Query tail latencies also fluctuate between repetitions.
These maxima should not be interpreted as a change in retained trace data.
The benchmark exercises native C++ production sources, which are unchanged;
Rust printf rendering is verified separately by the interoperability tests.
