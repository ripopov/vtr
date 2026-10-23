# Synthetic format benchmarks

These C++20 examples compare VTR with FST and VCD for RTL-style signal
streaming, and with FTR for transaction streaming. A mixed workload compares
one VTR file with FST+FTR and VCD+FTR pairs. The suite measures the performance
requirements in [the rationale](../vtr/docs/RATIONALE.md): complete write cost,
full decode cost, file size, peak memory, selective access, and producer cost.

## Build and run

Requirements: Linux, Python 3.9+, Git, a C/C++20 compiler, CMake 3.20+, and development packages for LZ4, Zstandard, zlib, and bzip2. For example,
Debian/Ubuntu packages are `build-essential cmake python3 git liblz4-dev libzstd-dev zlib1g-dev libbz2-dev`.
SystemC, Verilator, and simulator submodules are unnecessary.

From this directory:

```sh
python3 build.py
python3 run.py
```

`build.py` fetches pinned revisions of
[libfst](https://github.com/gtkwave/libfst) and
[LWTR4SC](https://github.com/Minres/LWTR4SC) into ignored `.build/deps`, builds
the native `vtr-cpp` library in release mode, and links `.build/benchmark`.
`python3 build.py --offline` reuses fetched revisions.
It refuses to overwrite modified dependency checkouts. `CC` and `CXX` select
compiler executables. Rebuild after modifying VTR or benchmark sources.

The **default measured suite has a 280-second deadline**, leaving room below
five minutes for report generation. First-time downloads and compilation are
setup and are excluded from this budget. Each subprocess receives the remaining
deadline; an overrun is terminated, exits unsuccessfully, and produces a clearly
marked incomplete report. A slow machine is not silently given smaller workloads
or fewer repetitions. Use `--scale-divisor 2` explicitly to halve durations on
such a machine; the report records this change.

Successful runs write [report.md](report.md) in this directory; this report is
tracked in Git. Each invocation also creates a new `results/<timestamp>/`
containing an archived `report.md` and `raw.json`. Failed or interrupted runs
save an incomplete report there and preserve the tracked report. Use `--report`
to choose a different destination for focused experiments or smoke checks.
The report has comparison tables for write/read throughput and
space, query p50/p95 latency, peak RSS, producer capture/close time, CPU time,
and write variability. Each workload has metrics as rows: waveform tables have
VTR, FST, and VCD columns; transaction tables have VTR and FTR columns. Mixed
workloads have VTR, FST+FTR, and VCD+FTR columns. Raw data includes every repetition, query latency,
validation digest, input size, dependency revision, compiler, and binary hash.
Files are deleted after validation unless `--keep-traces` is supplied.

```sh
# All adapters and queries, with short traces for a quick correctness check.
python3 run.py --scale-divisor 100 --repeats 1 --keep-traces --report results/smoke.md

# Full defaults, keeping artifacts in a named, new directory.
python3 run.py --output results/comparison --keep-traces

# Focused experiment; this still observes the deadline.
python3 run.py --profiles requests many-generators --cpu 2 --report results/transactions.md
```

## Workloads

| Profile | Signals | Generators | Simulated ticks | Behavior |
|---|---:|---:|---:|---|
| `core` | 64 | 0 | 480,000 | Fast small core: divided clocks, reset, valid, opcodes, PCs, cache-line addresses, data buses, quiet registers |
| `soc` | 4,096 | 0 | 24,000 | Wider hierarchy with 32 signals per module and varied clock divisors |
| `idle` | 4,096 | 0 | 48,000 | SoC signals active for 64 ticks per 1,024-tick period; retained state during gated intervals |
| `requests` | 0 | 8 | 120,000 | Bursty memory requests from eight sources, four sources per FTR stream |
| `many-generators` | 0 | 64 | 24,000 | Many independent sources with overlapping requests and out-of-order completion |
| `mixed` | 256 | 16 | 24,000 | Signals and transactions interleaved in the same timestamp loop |

Signals are 1-, 8-, 32-, and 64-bit binary values. Data includes high-entropy
payloads, correlated addresses, counters, periodic clocks, and sparse activity.
The producer suppresses unchanged values identically for every format and emits
initial state at tick zero. Values persist across idle windows.

Transactions issue every fourth tick with phase offsets and periodic idle
bursts. Lifetimes span 4–63 ticks, allowing multiple outstanding requests from
each generator and completion out of issue order. Every transaction preserves
its generator, begin/end timestamps, and five unsigned attributes: `serial`,
`address`, `bytes`, `opcode`, and `status`. FTR attaches status at completion and the other fields at issue. VTR captures
the five values as a typed array at issue, using the deterministic status from
the common schedule, and records the actual interval end at completion. All
readers verify the same five values and lifetime; neither adapter receives a
precomputed recording. Active lifetime state is bounded by
generator count and maximum latency, rather than total trace length. The final
drain completes requests that extend beyond the nominal duration.

These are deterministic synthetic traces, not recordings from an RTL simulator.
The small-core case emphasizes fixed producer costs; the wider and idle cases
exercise grouping, repeated structure, and selective reads. The multiple
generators are logical sources on one producer thread, not concurrent host
threads.

## Measurement and fairness

- All cases share one logical CPU selected from the current affinity mask.
  Format order is shuffled with a recorded seed for each of three full
  repetitions. No simulator computation is included beyond the common synthetic
  producer. The null-producer measurement provides a generation-cost reference;
  reported write times do not subtract it.
- VTR uses its native C++20 interface, LZ4, synchronous writing, and otherwise default writer
  settings. FST uses libfst's native writer with LZ4, compressed hierarchy, and
  parallel mode disabled. FTR uses the upstream `ftr_writer<true>` standalone
  backend with its normal 64 KiB block threshold. VCD uses a buffered text writer
  with standard binary-vector shortening. No writer receives precomputed output
  or an in-memory copy of the complete workload.
  FST and FTR share libfst's bundled LZ4 implementation; VTR uses the system LZ4 library.
- Complete write wall time includes file creation, declarations, producer
  generation, capture, compression, indexes, drain, and close. Capture and close
  are also reported separately. Writes use the filesystem page cache without
  `fsync`, so results measure buffered tracing rather than durable storage.
- Full reads decode all values/attributes and calculate count plus two
  order-independent fingerprints. Every read and every query is checked against
  a separate replay of the synthetic producer, including lifetimes and source
  identity. Completion order and file grouping may differ without changing the
  result. A mismatch stops the run and prevents a successful report.
- Every write, full read, and query batch has its own process. Peak RSS is Linux
  `/proc/self/status` `VmHWM`, which excludes inherited fork-time high-water
  marks; the report takes the maximum across repetitions. CPU time comes from
  `getrusage(RUSAGE_SELF)` and includes process startup. Wall timers are inside the
  executable. Read/query RSS includes file mappings and metadata caches.
- Reads follow writing, so the OS page cache is warm. Each of the twelve query
  probes opens and closes a fresh reader, charging metadata loading, native
  indexing, decompression, filtering, and result hashing. This is reader-cache
  cold, **not** storage-cache cold. No privileged cache flushing is performed.
- File size includes metadata and native indexes. Paired-format rows sum both
  files. Logical byte volume is explicitly defined as 8-byte time + 4-byte signal
  ID + packed binary value for a transition, or 8-byte identity + 4-byte generator
  + two 8-byte timestamps + five 8-byte attributes for a transaction. Bytes per
  observation is another normalization. Neither requires inventing a VCD-to-binary
  input size or treating text bytes as the producer's native payload volume.

## Queries and reader implementations

| Operation | Result | Access path |
|---|---|---|
| `search-signal` | All transitions of one signal equal to a target value in an inclusive time window | VTR signal/time selection; FST facility mask/time range; VCD sequential scan |
| `find-signal` | All matching transitions of one signal anywhere in the trace | Same readers, with no time-window restriction |
| `point` | Signal value already in effect at an arbitrary timestamp | VTR `value_at`; FST handle-at-time lookup; VCD scan up to the requested time |
| `search-transaction` | Transactions from one generator overlapping a window with `address == target` | VTR generator/time selection and typed-payload filtering; FTR stream/time chunk pruning and attribute filtering |
| `find-transaction` | All transactions from one generator with `address == target`, regardless of time | Same readers, with no time-window restriction |

Queries vary sources, timestamps, and signal widths. Some value predicates are
guaranteed absent; others have matches. An idle signal's point lookup still
returns its preceding state. Queries return all matches, so a reader cannot
win by stopping after the first one. Query percentiles cover 36 probes per
operation at default settings and include open/close costs.

FST and VTR use their library readers. LWTR4SC supplies a standalone FTR writer
and a Python dump utility, but no matching C++ reader. `ftr_reader.hpp` therefore
implements the CBOR/LZ4 subset emitted here: dictionaries, streams, generators,
and transactions with unsigned attributes. It scans chunk headers on open and
uses their stream/time bounds for pruning. That index construction is charged
on every query. It preserves the upstream writer's chunk boundaries and time
bounds. No attribute-value index is added to either format. `readers.hpp`
contains a streaming VCD reader for the binary-value subset emitted here.
These adapters are benchmark examples, not general-purpose trace readers.

Read speed is an implementation comparison as well as a format comparison.
The report states the reader choices explicitly. These workloads do not measure
four-state or wider-than-64-bit signals, rich stages/events/relations, owner
navigation, recovery, asynchronous worker scaling, queue backpressure, or
logging. Those require additional workloads; the synchronous single-CPU
comparison establishes the shared resource budget without claiming to validate
every design property in the rationale.

## C++ examples

`workload.hpp` contains the streaming producer and independent result oracle.
`writers.hpp` demonstrates native VTR C++, libfst, standalone FTR, VCD, and paired sinks.
`readers.hpp` demonstrates scans, predicates, and signal point lookup;
`ftr_reader.hpp` handles the FTR subset. `benchmark.cpp` exposes each operation
as a small command-line example that writes one JSON result:

```sh
.build/benchmark write vtr requests /tmp/requests.vtr 1
.build/benchmark read vtr requests /tmp/requests.vtr 1
.build/benchmark search-transaction vtr requests /tmp/requests.vtr 1
.build/benchmark oracle null requests /tmp/unused 1
```

The final argument divides profile duration. The Python runner performs the
cross-format checks and aggregates the output into tables. Custom experiments
should use the runner so that a decoding error cannot become a performance win.

Run `python3 -m unittest discover -s . -p 'test_*.py'` after building to check
all format/query combinations on short traces, truncated FTR rejection, and
deadline failure reporting.

## Implementation and performance boundaries

VTR uses the current C++20 typed SDK and VTR 3.2 pages. Its independent Rust
implementation reads and writes the same format. Codec and transform policies
may produce different physical files while preserving observations. Bidirectional
native tests check both implementations; this benchmark compares the named
native readers and writers rather than the Rust implementation's throughput.

Compare complete write/read time, file size, every query percentile, RSS, and
capture/close costs. Typed payloads and retained result ownership change both
storage and CPU work. Smaller files or a faster capture loop alone do not prove
an end-to-end improvement. Retain slower results and investigate their causes.
Fresh-reader queries include schema/hierarchy construction; their costs must
remain visible even when steady-state streaming improves.

A comparison against revision `a0fa762` used seven shuffled pairs per default
workload on CPU 0. Both producer replays and every read and
query digest agreed. The original executable's SHA-256 was
`9512050cc6af8e670b0c8dd5211d72119aa29f66dba6854a7f7d2679e97f7f91`;
the current executable's was
`3b0d101ed230757bca6fe0a865474175939829b7f205dc929b8046515d2fefbe`.
These measurements describe this host and configuration, not performance limits:

| Workload | Complete write time | Complete read time | File size | Write peak RSS |
| --- | ---: | ---: | ---: | ---: |
| core | +2.9% | −1.2% | +0.4% | −63.2% |
| soc | +13.6% | +14.8% | −0.9% | −85.8% |
| idle | +19.9% | +10.9% | −6.5% | −30.5% |
| requests | −2.7% | +35.0% | −54.4% | −34.7% |
| many-generators | −3.8% | +31.3% | −56.0% | −29.7% |
| mixed | +5.1% | +13.6% | −21.2% | −9.8% |

Two controlled probes distinguish API costs from waveform buffering. With the
same library, workload, codec, and 16 MiB buffer, direct wire, engine, and public
SDK writers produced equal-size files and independently verified observations.
Their median complete SoC writes took 348.08, 356.60, and 385.65 ms respectively.
The public API selects the execution backend and translates errors; the engine
also enforces poisoned-writer lifecycle rules. These layers remain in the
supported API. Increasing the buffer to 64 MiB reduced the default SoC write
time by 5.6% while increasing peak RSS from 16.23 to 41.00 MiB. Removing the byte
limit increased RSS to 115.11 MiB and made the write 3.1% slower. Buffer size
alone does not explain the CPU costs.

A separate seven-pair probe kept the public writer and replaced only the reader
adapter with the private engine API. Complete requests reads fell from 21.31 to
12.91 ms, and many-generators reads from 37.40 to 20.55 ms. The engine lends
transaction rows to a synchronous visitor; the SDK constructs independently
retained records and payloads. Public waveform reads additionally adapt samples
to `SignalChange`; SoC reads took 117.14 ms through the SDK and 104.35 ms through
the engine. These probes quantify different ownership and presentation
contracts, rather than interchangeable APIs. The benchmark continues to measure
the public SDK, including those costs.

Fresh-reader signal queries also pay for the public catalog. In the revision
comparison, SoC search-signal p50 rose from 0.270 to 0.415 ms, while core fell
from 0.426 to 0.109 ms. Transaction search p50 fell from 1.830 to 0.106 ms for
requests and from 1.837 to 0.291 ms for many-generators. The typed pages and
selective indexes favor different access patterns; smaller files and faster
transaction searches coexist with the retained-record read costs above.

## Paired implementation comparisons

To compare future native changes, preserve the benchmark binary before rebuilding:

```sh
cp .build/benchmark /tmp/vtr-before
# Modify the implementation, then rebuild.
python3 build.py --offline
python3 compare.py /tmp/vtr-before --baseline-label before --candidate-label after
```

`compare.py` runs both binaries on one CPU in shuffled pairs, verifies both
producer replays and every read/query digest, and archives the Markdown report
and raw data in `results/paired-<timestamp>/`. Its default seven repetitions
have a 280-second deadline. It supplements the full cross-format default suite;
the tracked [report.md](report.md) continues to come from `run.py`.
