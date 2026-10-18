# Native C++ VTR library

`vtr-cpp` is an independent C++20 implementation of the VTR 2.0 reader and
writer. It reads files produced by the [Rust library](../vtr/README.md), and
Rust reads its files. The native library and its C interface do not link Rust.
The [format specification](../vtr/docs/SPEC.md) defines their shared contract.

## Build, test, and install

The native implementation currently targets Linux. It requires a C++20 compiler
with standard-library formatting support, CMake 3.20+, and LZ4, Zstandard, and
zlib development packages. GCC 13 or newer is suitable. On Debian/Ubuntu:

```sh
sudo apt-get install build-essential cmake liblz4-dev libzstd-dev zlib1g-dev
cmake -S vtr-cpp -B build/cpp -DCMAKE_BUILD_TYPE=Release
cmake --build build/cpp --parallel
ctest --test-dir build/cpp --output-on-failure
```

Run these commands from the VTR repository root. Tests also require Python 3
and the repository's Rust toolchain. They compare both decoders on files
written by each implementation using every supported compression codec. They
also cover C callers, SystemVerilog/DPI tracking, ownership, repeated event
occurrences, long-lived transactions, recovery, malformed files, and Linux
termination handling. `-DBUILD_TESTING=OFF` builds the library without Rust or
Python.

Install into an independent prefix:

```sh
./vtr-cpp/install.sh --target-dir /tmp/vtr-build --prefix /tmp/vtr-install
```

The script installs `libvtr.a`, C/C++ and SystemVerilog headers, a CMake package,
and pkg-config metadata. `--debug` selects a debug build. Both paths are
resolved from the caller's working directory. CMake consumers use:

```cmake
find_package(vtr CONFIG REQUIRED)
target_link_libraries(simulation PRIVATE vtr::vtr)
```

Set `CMAKE_PREFIX_PATH` to the installation prefix. A C caller can link with:

```sh
export PKG_CONFIG_PATH=/tmp/vtr-install/lib/pkgconfig
cc -std=c99 $(pkg-config --cflags vtr) example.c \
   $(pkg-config --static --libs vtr) -o example
```

## C++ interface

[`include/vtr.hpp`](include/vtr.hpp) provides owning `Writer` and `Reader`
classes. This example records a signal and an attributed transaction, then
queries both:

```cpp
#include <vtr.hpp>
#include <iostream>

int main() {
    vtr::Writer writer("example.vtr");
    writer.metadata().timescale = -9;
    auto top = writer.add_scope(vtr::none, "top");
    auto signal = writer.add_var(top, "count", {vtr::SignalKind::bits, 32, 2}).second;
    auto generator = writer.add_generator(top, "request", "TRANSACTION");
    auto address = writer.intern("address");
    writer.set_time(10);
    writer.emit(signal, 42);
    auto tx = writer.begin_tx(generator, 10);
    writer.tx_attr(tx, address, std::uint64_t{0x1000});
    writer.end_tx(tx, 20);
    writer.close();

    vtr::Reader reader("example.vtr");
    reader.changes(signal, 0, vtr::forever,
        [](vtr::Time time, vtr::Id, const vtr::SignalValue& value) {
            std::cout << time << ": " << value.as_u64() << '\n';
            return true;
        });
    reader.transactions({generator, vtr::none, 0, 30},
        [](const vtr::Transaction& transaction) {
            std::cout << "transaction " << transaction.id << '\n';
            return true;
        });
}
```

A writer has one producer thread. Metadata is editable until its first flush;
node attributes are editable until that node's declaration is written. Strings
and hierarchy may grow during capture. Waveform times are nondecreasing;
transaction and log timestamps are independent and may arrive out of order.
Event signals preserve repeated values and repeated timestamps. An explicit
`close()` reports write failures; the destructor attempts to close but cannot
report them.

The writer is synchronous. Completed transactions go directly into the format's
column streams; reusable open-transaction storage avoids allocating for each
scalar attribute. Signal headers and values accumulate separately, with compact
binary packing, dynamic aliases, bounded compression runs, and sampled transform
selection. Waveform transformations and compression run at block flush, whereas
the Rust implementation prepares chunks during capture. Native `close()` can
therefore take longer even when capture and total write time are lower.
LZ4 is the default; stored and Zstandard blobs are also supported.
Every finished section is flushed to the operating system, without `fsync`.
`commit_interval_ms` optionally bounds buffering during continued producer calls.

Readers memory-map the file and select blocks by signal, generator, and time.
Individual compressed blobs must expand to less than 1 GiB.
Like the Rust reader, normal queries skip whole-section CRC scans. Set
`ReaderOptions::verify_crc` to verify each accessed section; C callers use
`vtr_reader_open_with_options` with `verify_crc = 1`. Directory checks and
recovery scans always verify available CRCs.
Do not modify or truncate a file while a reader maps it. Callbacks borrow their
arguments until they return; return `false` to stop. `load_signal()` returns
shared immutable storage that survives its reader and cache clearing. Queries
may run concurrently; `clear_cache()` requires exclusive access. Transaction
visits preserve file production order. Signal visits do not promise global
timestamp or cross-signal ordering. Values of one signal retain emission order.

## C and simulator interfaces

[`include/vtr.h`](include/vtr.h) exposes the native implementation through opaque
C handles. C API version 3 exposes the synchronous writer's supported settings.
Rebuild callers against this header. No Rust runtime or conversion layer is
involved. Input arrays and strings are copied during writer calls. Attribute
and callback pointers returned by readers are borrowed; loaded signal handles
share owning storage. Every owning handle has a matching close/free function.
`vtr_writer_close` consumes its handle even on error.

Status functions return `VTR_OK` or a `VTR_ERR_*` code. Constructors return
`NULL` or `VTR_NONE` on failure. Read `vtr_last_error()` on the calling thread
immediately after failure; success leaves its previous value intact. C++
exceptions are caught at every fallible C entry point.

`vtr_log.hpp`, `vtr_track.hpp`, `vtr_trace_dpi.hpp`, and `vtr_trace.sv` provide
logging, keyed transaction tracking, and SystemVerilog/DPI integration.
[Verilator integration](../integrations/verilator/README.md) documents the native
library's link dependencies.

The C interface includes hierarchy census, clock timeline queries, and activity
sidecars. Native activity generation runs on the calling thread and retains
exact stretches with a threshold of one time unit; it can use more space than the Rust builder's adaptive thresholds.
Its sidecars follow the same VTI format; the `threads` argument is currently a
parallelism hint and does not create worker threads. An explicit memory limit
bounds accounted index storage during build or decode; compression and I/O
buffers require additional memory. Cancellation prevents publication of an
unfinished index.

## Recovery and termination

A reader recovers complete, checksummed sections when a trailer is missing.
`vtr::recover(input, output)` preserves the complete sections and their IDs,
adds a recovery record, and seals a new file.

Linux C callers can install `vtr_guard_install` and register writers with
`vtr_guard_watch`. The guard handles fatal signals, cooperative stop requests,
and process exit. Initialize additional producer threads with
`vtr_guard_thread_init`. An interrupted writer call is recovered from its
published sections through a separate snapshot; an idle writer can finish its
buffered data. A deadline bounds best-effort crash handling. Allocator corruption,
SIGKILL, and failures before the first declarations are published may leave only
the previously completed sections recoverable. The Rust `vtr-guard` crate
remains the guard for Rust producers.

## Performance checks

The [benchmark suite](../benchmarks/README.md) calls the native C++ interface
and compares it with FST/VCD waveforms and FTR transactions. Run the full suite
and inspect every metric after changing either implementation or the format,
as required by [AGENTS.md](../AGENTS.md). Its tracked
[report](../benchmarks/report.md) identifies the measured implementation.
