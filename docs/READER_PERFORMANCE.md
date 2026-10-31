# Reader performance

The reader retains validated block directories, filters candidate blocks before
decompression, and validates nonmatching argument rows without retaining their
decoded values. Owners with child locators enter the bounded owner cache before
transaction callbacks. Expansion shares decompressed located pages through a
separate 8 MiB FIFO cache. These optimizations use the existing VTR 3.4 layout;
they do not retain an unbounded decoded copy of the trace. See
[query and lifetime semantics](TRANSACTIONS.md#7-queries-validation-and-lifetime).

## GPU instruction loading

Measured on macOS arm64 on October 31, 2026, with optimized native builds.
The baseline is `2b144b5`. The recording contains 121,744,801 bytes, 3,314
transaction blocks, 2,910,486 owners, and 2,910,473 relations. Each selected SIMD
slot contains 11,364 instructions; 1,044 owners reference located children.
The OS file cache is warm, and the first load uses a fresh reader cache.

| Operation | Baseline | Optimized |
|---|---:|---:|
| Volna slot 0 complete track load | 10,005.9 ms | 449.5 ms |
| Volna slot 1 complete track load | 10,660.4 ms | 447.3 ms |
| Rust generator-filtered owner query, slot 0 | 994.3 ms | 222.0 ms |
| Rust relation scan with incident-edge filtering | 1,065.7 ms | 77.5 ms |
| C++ generator-filtered owner query, slot 0 | 615.5 ms | 160.3 ms |
| C++ owner query with nested expansion | 1,675.8 ms | 267.1 ms |
| C++ relation scan | 626.1 ms | 48.7 ms |

C++ optimized figures are medians of three fresh-reader runs. Reader phases
exclude viewer conversion and indexing; the Volna figures include them. Both
slots' selective results match full-scan decoding for identity, classification,
timestamps, payload bytes, attributes, embedded detail, and locators. Expansion
retains 69 located pages using 3,173,159 bytes, below its configured limit.

## Default benchmark validation

The full default comparison suite passes before and after optimization on
Linux aarch64, using the same container image, compiler, CPU affinity 0,
three repetitions, scale divisor 1, and format-order seed. All independent
producer replays and file sizes match exactly for every backend. The generated
[default report](../benchmarks/report.md) contains every measured metric.

A separate seven-pair comparison runs the original and optimized binaries in
shuffled order on the same CPU, with complete default workloads and all query
probes. Changes below are candidate / baseline minus one.

| Workload | Write time | Full read time |
|---|---:|---:|
| core | +0.1% | +1.0% |
| soc | −0.3% | +1.1% |
| idle | +3.3% | −1.4% |
| requests | −2.0% | −4.4% |
| many-generators | −1.0% | −7.4% |
| mixed | −0.2% | −2.2% |

Query medians in the paired run range from −15.0% to +4.2%; query p95 changes
range from −33.4% to +2.2%. Both the original baseline and the optimized reader
show occasional process RSS jumps. Linux memory diagnostics identify 2 MiB
anonymous huge pages as a source of that variation; a single maximum is not
evidence of an equivalent increase in retained reader data. The intentional
retained-memory costs are block directories, bounded owner storage, and the
explicitly configured located-page cache. Owner charges include vector capacity
and recursively owned attribute buffers.

The common-format benchmarks do not exercise rich owner expansion or embedded
detail. Their near-neutral timings complement the GPU workload rather than
measuring the same access pattern.

Validation also includes all 40 native CTest cases, including Rust/C++
interoperability, the full Rust workspace, strict workspace Clippy, and Volna's
default test suite. Regression cases cover nested callbacks, cache eviction and
clearing, disabled caches, payload lifetime, dictionary state after skipped
rows and errors, full-record delivery at decoder boundaries, retained-capacity
admission, and selective results against the full decoder. The C++ allocation
failure sweep covers 19 admission failure points and verifies subsequent
expansion, embedded lookup, cache limits, and clearing.
