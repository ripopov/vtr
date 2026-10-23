# Verilator integration

`ext/verilator` pins the [Volna branch](https://github.com/ripopov/verilator/tree/volna)
of the Verilator fork, including its VTR waveform backend, structured logging,
SystemVerilog/DPI helpers. The fork is a submodule;
its source is developed in `ripopov/verilator`.

Build from this repository's root with a C++20 compiler, make, CMake, autoconf,
flex, bison, Perl, and help2man installed:

```sh
git submodule update --init --recursive ext/verilator
cargo build --release -p vtr-cli
cmake -S vtr-cpp -B build/cpp -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF
cmake --build build/cpp --parallel
integrations/verilator/build.sh "$PWD/build/verilator/install"
```

The build helper installs the matching `vtr_trace.sv`, `vtr_trace_dpi.h`, `vtr_track.h`,
`vtr_common.h`, and `vtr_writer.h` alongside Verilator. Model builds use the C++20 headers and library:

```sh
build/verilator/install/bin/verilator --cc --exe --trace-vtr top.sv main.cpp
make -C obj_dir -f Vtop.mk VTR_INCLUDE="$PWD/vtr-cpp" VTR_LIBDIR="$PWD/build/cpp" \
  USER_LDLIBS="-lvtr -llz4 -lzstd -lz"
```

The harness includes `verilated_vtr_c.h` and uses `VerilatedVtrC` in place of
`VerilatedFstC`. CMake integrations can pass `TRACE_VTR` to `verilate()`.
See the [C++ guide](../../vtr-cpp/README.md) for installation, typed recording,
and recovery. The pinned backend supports inline recording and threaded producers;
its C++ crash-guard option is unsupported.

Run the simulator smoke test after building the C++ library and CLI (FST comparison
requires pkg-config and the LZ4 development package):

```sh
python3 integrations/verilator/smoke/run.py --verilator build/verilator/install/bin/verilator
```

The test builds the same hierarchical register model with VTR and FST, converts
FST to VTR, and compares every output and child alias sample with expected values.


Generate and verify structured-log fixtures for both execution modes:

```sh
python3 integrations/verilator/logs/run.py --verilator build/verilator/install/bin/verilator
cargo test -p vtr --test verilator_logs
```

The generator exercises HDL partial writes, severity metadata, `$error`, `$fatal`,
`$finish`, binary output, runtime diagnostics, and repeated dumps. Build products
and VDB sidecars remain under `build`; only the VTR log fixtures are copied into
the reader regression inputs. The fork's pinned revision is recorded by the
`ext/verilator` gitlink, including its own slang dependency pin.
