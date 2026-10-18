# Verilator integration

`ext/verilator` pins the [Volna branch](https://github.com/ripopov/verilator/tree/volna)
of the Verilator fork, including its VTR waveform backend, structured logging,
SystemVerilog/DPI helpers, and optional crash guard. The fork is a submodule;
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

The build helper installs the matching `vtr_trace.sv`, `vtr_trace_dpi.hpp`, and
`vtr_track.hpp` alongside Verilator. Model builds use the C API headers and library:

```sh
build/verilator/install/bin/verilator --cc --exe --trace-vtr top.sv main.cpp
make -C obj_dir -f Vtop.mk VTR_INCLUDE="$PWD/vtr-cpp/include" VTR_LIBDIR="$PWD/build/cpp" \
  USER_LDLIBS="-lvtr -llz4 -lzstd -lz"
```

The harness includes `verilated_vtr_c.h` and uses `VerilatedVtrC` in place of
`VerilatedFstC`. CMake integrations can pass `TRACE_VTR` to `verilate()`.
See the [C API guide](../../vtr-cpp/README.md) for installation and crash recovery,
and the [logging guide](../../vtr/docs/LOGGING.md) for typed simulator messages.

Run the simulator smoke test after building the C API and CLI (FST comparison
requires pkg-config and the LZ4 development package):

```sh
python3 integrations/verilator/smoke/run.py --verilator build/verilator/install/bin/verilator
```

The test builds the same hierarchical register model with VTR and FST, converts
FST to VTR, and compares every output and child alias sample with expected values.
