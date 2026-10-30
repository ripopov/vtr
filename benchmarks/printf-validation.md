# C++ printf validation

Validation used native ARM64 Linux, GCC 15.2, glibc 2.43 and the dependencies
pinned by `benchmarks/build.py`. The baseline was `66884933bd199a138fc4852eb5b4e8b354bf3b35`.
The committed report previously used x86-64, so a fresh unchanged-baseline build
provided the same-machine comparison. The full default suite ran at scale 1
with three repetitions and CPU affinity 0; every replay and query check passed.
The refreshed [default report](report.md) records all formats and metrics.

A separate seven-pair comparison used `benchmarks/compare.py`, full workload
sizes, shuffled binary order and the same CPU. Every metric was reviewed,
including file size, capture/close time, RSS and query p50/p95. Below are
candidate / baseline changes; these small timing differences are not speedup
claims. File sizes and replay/query results were unchanged.

| Workload | Write time | Read time | Capture ns/tick |
|---|---:|---:|---:|
| core | -0.9% | -0.3% | -1.0% |
| soc | -1.7% | -0.6% | -0.9% |
| idle | -0.8% | +1.4% | +0.6% |
| requests | +0.4% | +0.2% | +0.4% |
| many-generators | +0.3% | -0.1% | +0.2% |
| mixed | -0.4% | +0.5% | -0.7% |

Small-process peak RSS varied by roughly 2 MiB in either direction. A further
seven-pair control comparing the baseline binary with itself reproduced this
variation: mixed transaction-search RSS differed by +39.5%, and requests read
RSS by -27.9%. The control also showed an 18.8% many-generators read-time
variation. Repeated paired results did not identify a reproducible performance
regression attributable to the printf implementation. These workloads check
existing functionality; they do not benchmark printf rendering throughput.

The printf test covers 5,017 cases in each of inline and threaded mode, including
libc reference comparisons, typed-width extensions and invalid inputs. It also
checks 5,000 generated format strings, token/output limits, borrowed input
lifetimes, callsite reuse, metadata dialect selection and raw capture. The test
passed with ASan/UBSan on Linux. All 38 applicable native CTest entries,
including existing bidirectional Rust/C++ interoperability, passed, as did the
installed-package consumer. The new printf test is included in that count.

The standalone `scalar_format_reference` entry fails against the existing
brace-format fixture on the available standard libraries. Building that oracle
from the unchanged baseline reproduces the failure and generates byte-identical
output to the candidate oracle. This entry does not link VTR. No Rust source or
fixture changes are included.
