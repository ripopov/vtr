# Verilator `--trace-vtr`: waveforms and RTL VDB

The `ext/verilator` submodule pins a commit from the
[`vtr` branch](https://github.com/ripopov/verilator/tree/vtr), based on
Verilator 5.050. It includes a VTR trace backend next to the existing `--trace-vcd`, `--trace-fst` and `--trace-saif` formats. It is a
port of Verilator's FST backend (`verilated_fst_c.{h,cpp}`) onto the VTR
C API (`core/vtr-capi/include/vtr.h`), so a model built with `--trace-vtr`
runs exactly the same generated trace code as with `--trace-fst`; only the
sink differs.

`--trace-vtr` also exports the elaborated RTL into `<Mdir>/<prefix>.vdb.json`,
before optimization removes source structure. The generated model carries this
companion and writes it beside each recording (`simulation.vtr` becomes
`simulation.vdb.json`). It records a shared design identity and explicit trace
mapping, enabling source navigation, annotated netlist SVGs, and temporal driver
tracing with the existing `vtr-vdb` CLI. No Python dependency is needed for
native export. See [VDB_RTL.md](../../docs/VDB_RTL.md) for the schema and limits.

After writing the companion, Verilator runs `verilator_vdb_index` from the
directory holding `verilator_bin`. That program (`src/vdb_index` in the fork)
elaborates the same file set with the [slang](https://github.com/MikePopoloski/slang)
release pinned as the fork's `ext/slang` submodule and appends the VDB
`source_index`: every token of every source file with its class, modifiers and
declaration, plus the generate blocks each instance leaves uninstantiated.
Surfer's source tile renders highlighting, hover values, ctrl-click navigation
and alt-click adding of signals from that section alone. A missing or failing
indexer raises Verilator's `VDBINDEX` warning and the VDB ships without the
section.

Build prerequisites include a C++20 compiler, make, cmake, autoconf, flex,
bison, Perl, and help2man. Configuring the indexer fetches the `fmt` library
with CMake's FetchContent unless a system `fmt` (12.1 or newer) is installed.

```sh
git submodule update --init --recursive ext/verilator
integrations/verilator/build.sh bench/build/verilator/install   # build and install pinned source
cargo build --release -p vtr-capi                                # libvtr.a / libvtr.so + vtr.h
verilator --cc --exe --trace-vtr top.sv main.cpp
make -C obj_dir -f Vtop.mk VTR_INCLUDE=$VTR/core/vtr-capi/include VTR_LIBDIR=$VTR/target/release
```

In the harness, use `VerilatedVtrC` where you would use `VerilatedFstC`
(`#include <verilated_vtr_c.h>`; `verilated_vtr_sc.h` provides the SystemC
wrapper). `verilated.mk` adds `-I$(VTR_INCLUDE)`, `-L$(VTR_LIBDIR)` and
`-lvtr` when `VM_TRACE_VTR=1`; with a static `libvtr.a` no runtime path is
needed. CMake users get `TRACE_VTR` on `verilate()`.

`build.sh` also copies `core/vtr-capi/include/vtr_trace.sv`,
`vtr_trace_dpi.hpp` and `vtr_track.hpp` into the installed `include/vtr/`, so
every install carries the package of the VTR revision it was built from.

`build.sh` configures Verilator with clang++ when one is installed (set
`CXX` to override): the host compiler is inherited by every model through
`verilated.mk`, and on the benchmark's C910 model gcc generates code that
runs 3x slower than clang's from identical sources unless PGO is used
(`docs/BENCHMARKS.md`, host compiler section).

What the VTR branch adds:

* `src/V3EmitVdb.cpp`: the RTL VDB export; it runs `verilator_vdb_index` on
  the written companion and embeds the indexed document in the model.
* `src/vdb_index/`: the indexer, its CMake build against `ext/slang`, and its
  tests (`make -C <build-dir>/src vdb_index_test`); `src/Makefile.in` builds
  it through a CMake sub-build when `cmake` is found and installs it beside
  `verilator_bin`.
* `src/V3Options.{h,cpp}`: the `--trace-vtr` switch, class base
  `VerilatedVtr`, runtime source `verilated_vtr_c.cpp`; the one-format-only
  check includes VTR.
* `src/V3EmitMk.cpp`, `src/V3EmitMkJson.cpp`, `include/verilated.mk.in`,
  `verilator-config.cmake.in`: `VM_TRACE_VTR` plumbing and link flags.
* `src/V3EmitCImp.cpp`: enum data types are emitted for VTR like for FST
  (VTR files carry enum tables; the var refers to its table through an
  `enum_table` attribute).
* `include/verilated_vtr_c.{h,cpp}`, `include/verilated_vtr_sc.h`: the
  backend. Scopes map to VTR scope types with the FST codes (module, struct,
  union, interface, sv_array), var types and directions use the FST codes,
  names follow the FST backend (`name[index]`, `name [msb:lsb]`), aliases
  become VTR aliases, values are two-state (Verilator models are two-state)
  and are emitted with `vtr_writer_emit_bit/u64/words/real`. The writer runs
  with its own value deduplication off, since Verilator's generated code
  already emits only changed values; every other option is the VTR default
  (zstd level 3, background encoder).
* `src/V3Global.cpp`, `src/V3ParseImp.cpp`, `src/V3File.{h,cpp}`,
  `src/V3EmitVdb.cpp`: with `--trace-vtr`, the `vtr_trace` SystemVerilog
  package (`include/vtr/vtr_trace.sv`) is parsed after the `std` package and
  removed again, with its dependency and VDB source entries, when the design
  never names `vtr_trace`.
* `include/verilated_vtr_dpi.cpp`, compiled with every `--trace-vtr` model:
  the package's DPI bodies (`include/vtr/vtr_trace_dpi.hpp`) over a sink
  implemented by `VerilatedVtr`: one runtime per simulation context, the
  model name as `$root`, scope nodes from the trace prefixes, replay at open
  of what was declared before, and misuse warnings in the simulation log.
* `docs/guide/*.rst`: option documentation.

The benchmark suite (`bench/run.py`) builds this Verilator into
`bench/build/verilator/` and uses it for the simulator-integrated
measurements (`docs/BENCHMARKS.md`).

Build products live in `<prefix>/../build` by default; pass a second argument to
select another build directory, and set `JOBS` to control build concurrency.
The script uses the checked-out submodule revision and performs no network clone
or patch application. To reproduce a VTR checkout, use `git submodule update
--init --recursive ext/verilator`; `--remote` intentionally selects a newer
branch revision.

After building the CLI and C library, run the integration smoke test (the FST
comparison also needs pkg-config and the liblz4 development package):

```sh
cargo build --release -p vtr-cli -p vtr-capi
python3 integrations/verilator/smoke/run.py \
  --verilator bench/build/verilator/install/bin/verilator
```

It builds the same hierarchical register model using VTR and FST, converts the
FST recording to VTR, and checks both output and child alias values against
independent expected samples. Build logs and recordings remain under
`bench/build/verilator-smoke`.

For VDB integration tests, also build `cargo build -p vtr-vdb`, then run:

```sh
python3 integrations/verilator/vdb/run.py \
  --verilator bench/build/verilator/install/bin/verilator
```

Clocks declared through the `vtr_trace` package (see
[vtr_clocks.html](../../docs/vtr_clocks.html)) have their own suite:

```sh
python3 integrations/verilator/clocks/run.py \
  --verilator bench/build/verilator/install/bin/verilator
```

It builds a testbench with a DVFS clock and a gated clock in one and two
thread modes and checks that the recorded stretches reproduce the rising edges
of the dumped waveforms, the scope paths, unit conversion and misuse warnings,
a clock started before the file opens, a signal-free open, and that a design
naming nothing from the package gets no package code.

Pipeline tracers written against the package's tracker part (see
[c910-verilator-tx-stream.html](../../docs/c910-verilator-tx-stream.html))
have their own suite:

```sh
python3 integrations/verilator/pipeline/run.py \
  --verilator bench/build/verilator/install/bin/verilator [--update-example]
```

A tracer bound to a small scripted core exercises every tracker call. In one
and two thread modes the script compares every transaction, stage, status,
attribute, relation, parent and time with an exact expected list, checks that
stage boundaries fall on edges of the declared clock, that the waveforms equal
those of the design built without the tracer, the misuse warning of a
deliberate stale key, and a signal-free open. `--update-example` writes
`volna/volna/examples/pipeline_demo.vtr`, which a Volna test opens.

A tracer passes the hardware's own identifiers as keys; they are narrower than
the 64-bit `vtr_key_t`, so tracer modules turn off `WIDTHEXPAND` around
themselves (the C910 build already runs with `-Wno-fatal`).

The VDB suite checks source locations and elaborated hierarchy, real-simulator
pipeline provenance and RTL expressions, automatic attachment and identity
rejection, partial recordings, annotated SVGs for every module, and the
`source_index` the indexer appended to every companion.
