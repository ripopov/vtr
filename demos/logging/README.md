# Logging demo

Two small programs show simulator-style logging into a VTR file and reading it
back: `sim_log_demo.cpp` uses the C++ header `vtr_log.hpp` over the C API, and
`crates/vtr/examples/logging.rs` uses the Rust API. Both model a SoC with a CPU
and a DMA engine; each component owns a `LOG` stream, the DMA transfers are
transactions, and the DMA messages are linked to the transfer they belong to.

```sh
cargo build --release                                   # libvtr.a and the vtr CLI
cmake -S demos/logging -B build/logging && cmake --build build/logging
./build/logging/sim_log_demo demo_cpp.vtr
cargo run --release --example logging -- demo_rust.vtr

vtr log demo_cpp.vtr --severity warn                    # render warnings and errors
vtr log demo_cpp.vtr --stream soc.dma.log --from 0 --to 4000
vtr log demo_cpp.vtr --sites                            # the call sites (generators) with their argument types
vtr tx demo_cpp.vtr --stream soc.dma.transfers --max 3  # the transactions the messages link to
```

The C++ side logs with `VTR_LOG(stream, severity, time, "fmt {}", args...)`
(or the `VTR_LOG_INFO`/`VTR_LOG_WARN`/... shorthands). The first execution of a
statement registers its call site: format string, severity, file, line,
function and the argument types deduced from the C++ types. Every execution
then stores only the timestamp and the raw argument values. The reader
formats on demand; see `docs/LOGGING.md`.
