# Typed VTR for C++20

A standalone CMake project for typed transactions, located stages and events,
logs, clocks, waveforms, recovery, and VTI 3.0 activity indexes. The public
interface is in `vtr_common.h`, `vtr_writer.h`, and `vtr_reader.h`, in the
`vtr` namespace. Reader and writer headers depend only on the common header
and the C++ standard library.

The implementation follows the consolidated [format specification](../vtr/docs/SPEC.md)
and [typed record specification](../vtr/docs/TRANSACTIONS.md). All implementation sources
and vendored headers are contained in this directory. Builds do not download
source code or depend on another VTR checkout. Files use VTR 3.2; activity sidecars
use the source-bound VTI 3.0 format. The C++ library version is 4.0; its producer
interface and package version are independent of the file-format version.

## Build and test

Use Linux or macOS, CMake 3.20 or newer, a C++20 compiler and standard library
supporting `std::format`, and development packages for zlib, LZ4, and Zstandard.
GCC 15 and Clang 21 with libstdc++, and Apple Clang 17 with libc++, are tested.
Python 3 runs the independent decoding tests;
it is unnecessary when configuring with `-DBUILD_TESTING=OFF`.

```sh
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build --parallel
ctest --test-dir build --output-on-failure
```

`scalar_format_reference` compares the complete scalar-format corpus against
libstdc++ output. The tested Apple Clang 17/libc++ toolchain differs in 111 of
its 5,368 cases,
including alternate general floats, subnormal hexadecimal floats, character
format flags, and Unicode width. This comparison remains a failing gate on
macOS; the API, wire-format, and Rust/C++ interoperability checks are independent
of these presentation differences.

`-DVTR_SANITIZE=ON` enables address and undefined behavior sanitizers.
`-DVTR_THREAD_SANITIZE=ON` enables ThreadSanitizer instead.
`-DVTR_BUILD_EXAMPLES=OFF` omits the tutorial executables.
`-DBUILD_SHARED_LIBS=ON` builds a shared library.

In the repository checkout, an additional Rust waveform audit checks both direct
assembly and SPSC recordings with an independent decoder:

```sh
cargo build --locked --manifest-path tests/rust_wave_oracle/Cargo.toml --target-dir build/rust-wave-oracle
python3 tests/engine/waveform_oracle.py build/waveform_workers build/rust-wave-oracle/debug/vtr-wave-oracle build/wave-oracle-fixtures
```

This optional audit uses the adjacent Rust project. The standalone CMake build
and its regular tests do not require Rust.

Consumers can use `add_subdirectory` and link `vtr::vtr`, or install it:

```sh
cmake --install build --prefix "$PWD/install"
```

```cmake
find_package(vtr 4.0 CONFIG REQUIRED)
add_executable(simulator main.cpp)
target_link_libraries(simulator PRIVATE vtr::vtr)
```

Set `CMAKE_PREFIX_PATH` to the installation prefix when configuring a consumer.
The installed headers do not expose codec, container, or compression-library
types. Compression and thread dependencies are carried by the CMake target.

The optional `vtr_track.h` header provides keyed transaction tracking over the
same writer. It adds no library dependencies.
`vtr_trace.sv` and `vtr_trace_dpi.h` provide SystemVerilog instrumentation and
its C++ simulator adapter; the adapter also requires the simulator's `svdpi.h`.

## Tutorials

Run these commands from the build directory. Each example is a complete program,
accepts an optional output/input filename, and is run by CTest.

| Program | What it demonstrates |
|---|---|
| `./vtr_write_transactions run.vtr` | Schema binding, hierarchy, an ordinary owner, a located stage, and an event |
| `./vtr_read_transactions run.vtr` | Opening a file, finding a generator, a time query, typed payload decoding, owner lookup, expansion, and validation |
| `./vtr_waveforms` | Binary/four-state/real signals, aliases, time updates, point queries, transition scans, and activity sidecar round trips |
| `./vtr_parallel_recording` | Independent SPSC producers, simulation-time advances, nested owning payloads, statistics, and retained records |
| `./vtr_relationships` | Ordinary children, attributes, embedded stages, typed formatted logs, relations, clock associations, and cycle queries |
| `./vtr_logging` | Quill-style calls with static callsite schemas, source metadata, typed argument capture, and deferred formatting |
| `./vtr_recovery` | Explicitly incomplete output, recovery to a different file, and validation of surviving evidence |

The transaction tutorials share only `examples/instruction.h`, which declares
the data record and its schema. The reader opens its input directly and can run
in a separate process from the writer.

`examples/logging.cpp` uses the `LOG_INFO(logger, "request={}", request)` and
`LOG_WARNING` macros in `examples/logging.h`. Each callsite has static format,
severity, and source location metadata plus a typed slot. Callers with the same
argument types share a schema and encoder. The logger binds a source on first use;
subsequent calls index its source table directly, without hashing or runtime type
checks. Metadata registration is kept off the repeated capture path. Explicitly
declared typed callsites can use `Logger::prepare` before a latency-sensitive loop
to register a source without emitting a warmup message.

Format strings are checked against the argument types at compile time.
`Logger::set_level` filters calls before evaluating their arguments, including
`Severity::off`. Each logger belongs to one writer and logical producer; serialize
its calls. Text arguments are copied before the call returns;
`Reader::format_log` renders messages after recording finishes.

## Recording

Define an aggregate and specialize `Schema<T>` with a `name` and a tuple of
`field("name", &T::member)` entries in member declaration order. Enum bindings
provide a `name` and an `enumerators` array of `Enumerator<E>{"name", value}`.
Fixed-width scalars, owning or borrowed text, byte and logic vectors, time,
addresses, fixed-point values, arrays, and nested sequences compose recursively.
Spans provide borrowed sequence inputs; readers decode those sequences into owning vectors.
Capture serializes members without object padding and copies borrowed inputs
before returning. Unknown enum numeric values remain intact.

Declare metadata immediately after construction, before binding types or
creating sources. Intern attribute keys first. Set clock associations before
recording or flushing publishes the associated source. Further immutable
scopes, types, sources, and signals may be declared during recording.

`begin` returns a typed handle. End ordinary owners and located stages explicitly;
destroying a handle emits no observations. Ordinary
owners alone convert to `OwnerRef`. Handles and sources must be used with their
originating writer while it is alive. Ordinary children have independent
lifetimes. Ending an owner closes its active located stages, and early end times
are clamped to the corresponding begin. Repeated events and overlapping visits
remain distinct. Waveform time is monotonic and independent of record times.

Select execution at construction:

```cpp
vtr::Writer direct("direct.vtr", {.mode = vtr::ExecutionMode::inline_});
vtr::Writer background("background.vtr", {.mode = vtr::ExecutionMode::threaded});
```

Inline mode runs capture, assembly, compression and output on the calling
simulator thread. It creates no transport or encoding queues or worker threads,
and takes no tracing locks. One implicit producer owns the writer; callers
serialize access. Buffer settings are unused, and nonzero encoding workers are
rejected. `flush()` completes output synchronously.

Threaded mode is the default. One backend thread drains an SPSC queue per logical
producer. A caller without explicit registration uses an implicit producer.
Set recording time with `advance_time(T)`;
`begin`, `end`, `event`, and `log` use that time. The corresponding `*_at` methods
accept explicit observation times for historical activity without moving the
producer clock backward. Borrowed payloads are copied before return.

For parallel recording in threaded mode, register producers with `add_producer()` before starting
workers, then select one with `use_producer(id)`. Each signal and transaction
has one tracing owner per recording timestamp. An advance certifies that no
more observations below T can arrive from that producer. The backend completes
time only after all producers have advanced past it. Idle producers still
publish progress, and each producer calls `finish_producer()` when permanently
done. `release_producer()` permits another thread to attach to that logical
producer; it does not close the current timestamp.

In threaded mode, `BufferOptions` controls logical byte-ring capacity per producer and shared
pending memory. Defaults are a 256 KiB ring and 16 MiB of pending memory. Each
ring allocates twice its logical capacity for contiguous wraparound records.
Operations publish individually; a payload larger than the ring uses an owned
allocation, with at most one outstanding per producer. Small typed payloads
encode in 256 bytes of inline storage without a producer allocation.
Larger typed values encode directly into the reserved queue storage, avoiding
a temporary allocation and copy while retaining copied-input lifetimes.
The collector drains future observations into staging independently of their
eligibility for serialization, spilling beyond the memory budget. Queue
backpressure never discards observations. Active transactions, schemas, and
encoding buffers have separate memory costs. See [TRANSPORT.md](TRANSPORT.md)
for the ordering protocol and queue-design references.

Threaded recording calls publish every operation before returning. `publish()` checks
that the selected producer is usable and does not close its timestamp.
`flush()` captures queue positions and drains eligible observations and their
encoding work. With one producer, FIFO order permits draining its current
timestamp too. `close()` joins workers and
writes a complete directory and trailer. Unfinished ordinary owners are recorded
as open at close. `seal()` retains explicitly incomplete evidence and is valid
after failure. Recording semantic failures poison the stream; asynchronous
failures surface on a subsequent call or barrier. Do not retry accepted IDs.
Destruction attempts to seal, but explicit completion is needed to observe errors.
Buffered completion does not promise `fsync` durability.
Release all other threads' producers before closing or sealing. With the default
zero encoding workers, the recording backend also encodes and writes the file.
Nonzero `EncodingOptions::workers` adds that many encoders and one ordered output
thread; queue job and byte limits bound their outstanding work.

Completed records and transactions flush at a 512-KiB source-buffer target or
65,536 records; raw pages target 64 KiB. Smaller blocks reduce encoding bursts,
peak memory and selective-query work, at the cost of some compression efficiency.
Set `EncodingOptions::block_bytes` to tune that tradeoff for a workload. Waveform
buffers have a separate 16-MiB source target.

### HDL waveform declarations

Scope declarations retain HDL scope kinds, packed/array metadata and optional
component names. Signal and alias declarations retain variable type and
direction. `enum_table()` declares the literal names and exact bit strings of
an HDL enumeration independently of signal identity.

`emit_words()` copies binary values from least-significant-word-first 32-bit
storage, with an explicit bit width; unused high bits are ignored. Threaded
capture copies directly into queue storage and leaves alphabet conversion to
the backend. `EncodingOptions::waveform_deduplicate` may be disabled when a
simulator already filters unchanged values. Event variables always preserve
repeated occurrences, regardless of this option.

`Reader::waveform_time_range()` returns the first and last waveform timestamps,
including explicit advances with no signal update. Signal-free progress retains
its range endpoints in bounded storage. Change visitors return storage order; callers comparing multiple signal histories must track time
separately for each signal.

### Keyed transaction tracking

`Tracker` follows an item through reusable simulator keys, such as fetch slots,
instruction IDs and request queues. Each open item owns an ordinary transaction;
keys can change without changing its record ID.

```cpp
#include "vtr_track.h"

vtr::Writer trace("pipeline.vtr");
vtr::Tracker pipeline(trace, trace.transactions({.name = "instructions"}, "PIPELINE"), "cpu");
const auto fetch = pipeline.keyspace("fetch");
const auto issue = pipeline.keyspace("issue");
trace.advance_time(10);
(void)pipeline.open(fetch, 7, "IF", 10);
trace.advance_time(11);
pipeline.bind(fetch, 7, issue, 3, 11);
pipeline.stage(issue, 3, "EX", 11);
pipeline.attribute(issue, 3, "pc", std::uint64_t{0x80000000});
trace.advance_time(12);
(void)pipeline.close(issue, 3, vtr::Status::ok, 12);
trace.close();
```

`bind` adds an alias; `bind_oldest` moves the oldest items out of a keyspace and
can fuse several into one destination key. Operations on a fused key apply to
every member. Closing an item removes all its aliases. Reusing an occupied key
aborts its former item; binding onto an occupied destination aborts members
outside the incoming group. Keyspace handles belong to their tracker.

Stage changes close the previous visit on that lane. Repeating the current
stage preserves its original begin; an empty stage ends the lane. Repeated
events retain distinct IDs. Attributes and labels keep their last assignment
and publish once per key when the item closes, aborts or detaches. Close or abort
every live item, or call `detach`, before destroying the tracker. `detach` preserves unfinished
owners and stage ends for writer finalization. Parentage is fixed by the optional
parent passed to `open`; `owner` selects the oldest member of a fused key.

A tracker belongs to one logical producer. Its caller selects and advances that
producer, and uses the writer's release/attachment protocol when moving between
threads. The tracker uses no locks or background work. Item, key-table, group
and snapshot storage is reused up to the live population's high-water mark.
Repeated names use a bounded intern cache; new names, larger populations,
diagnostics and explicit `records` queries may allocate. Writer errors propagate;
invalid key operations accumulate diagnostics available through `take_diagnostics`.

### SystemVerilog instrumentation

`vtr_trace.sv` supplies clocks, pipelines, reusable keyspaces, stages, events,
attributes, relations and child creation with a fixed parent. Include
`vtr_trace_dpi.h` in exactly one simulator translation unit to define its DPI
entry points. Implement the `vtr_trace::Host` time/scope interface, the
`vtr_trace::Sink` writer attachment, and the `runtime()` and fatal `failure()`
hooks. Call `Runtime::opened()` and `closing()` around each recording;
`abandon()` releases SDK references after a failed recording.

`vtr_item_open_child(childspace, key, stage, parentspace, parentkey)` creates
an ordinary child at the current trace time. Use
`vtr_item_open_child_at(childspace, key, stage, parentspace, parentkey, time)`
when the child's first stage began earlier, such as at the previous pipeline
edge. The explicit time uses the same trace ticks as `vtr_now()`. Both calls
require a live parent in the same recording and fix its identity at creation;
closing the parent or reusing its key does not change the child's parent.

Calls into one runtime must be serialized. They may migrate between physical
threads: the sink's `enter()`/`leave()` pair selects and releases the same
logical producer around each call. The adapter creates no queue, lock or worker
of its own. Concurrent simulator instrumentation can use separate logical
producers through `Writer`; it must not call a shared SDK runtime concurrently.
An unused SDK does not advance waveform time when the recording closes.

`backend_cpu_nanoseconds()` samples owned collector and encoding workers on Linux
and retains their totals after exit. Other platforms report `std::nullopt`;
inline execution reports zero. Queue wait durations measure wall time.

## Reading

`find_signal()` and `find_generator()` select the requested declaration category,
so a clock generator and its dumped waveform can share a path. Path traversal
follows grouping scopes even when a signal has the same name as a scope.

`time_range()` includes waveform progress, owners, located records, and embedded
detail outside owner intervals. It decompresses pages containing embedded detail
and scans their time columns without materializing payloads or attributes or
filling the owner cache. Pages without embedded detail use indexed bounds.
`waveform_time_range()` remains a metadata-only query for waveform timestamps.
Owner-interval queries retain their recorded lifetime semantics.

`transactions()` visits ordinary owners and clock stretches; `records()` visits
located stages, events, and logs. Visitors return `Flow::next` or `Flow::stop`.
Queries use closed windows. Positive-duration stages match `begin <= last` and
`end > first`; point events match either endpoint. Storage order need not be
timestamp order. `lookup()` includes embedded records. `expand()` follows the
owner's child-block locators; ordinary children and logs have separate queries.

Copying a `Record` or `PayloadView` retains immutable storage, including schema,
after cache eviction and reader destruction. `payload()["field"]` and `[index]`
inspect dynamic values; `as<T>()` reconstructs owning C++ values and rejects
borrowed members. Aggregate bindings follow declaration order; non-aggregate
bound types must be default constructible. Decoding uses the reader's
`max_payload_bytes` as an allocation budget. File-local string IDs remain IDs;
resolve them with `Reader::string()` while the reader is alive.

Reader calls require external serialization. Checksum verification on access is
enabled by default. `validate()` additionally checks every page, global IDs,
ownership, references, locator membership, and waveform invariants. In incomplete
files it reports unresolved references; a complete file with those references is
corrupt. Recovery preserves surviving identities and marks its rebuilt directory
incomplete. A recovered file invalidates old activity sidecars.

Log formatting supports automatic and explicit numeric placeholders, escaped
braces, and C++20 scalar format specifications. Invalid placeholders/specifications
render as `{?}`. Byte arguments render in hexadecimal; fixed-point arguments
render as raw integer plus decimal exponent. The original format and typed
arguments remain available independently of rendering.

## Implementation and verification

`src/engine` owns schemas, VTR 3.2 record/TX pages, recording transport, lifecycle, navigation,
and full-file validation. `src/wire` owns container integrity and waveform
encoding. These are private implementation modules; clients use the public
headers. `third_party/unordered_dense` is the only vendored dependency and retains
its upstream license and checksums.

Tests exercise the public interface with every codec and encoding worker setting, nested
payloads, exact floating-point bits, repeated events, owner lifetimes, queries,
recovery, retained storage, and concurrent producers. Private codec tests include
independent Python decoders, malformed input, failure atomicity, bounded queues,
cache limits, and worker-independent wire output. See [VALIDATION.md](VALIDATION.md)
for the recorded verification and benchmark comparison.
