# Verilator `--trace-vtr`

The `ext/verilator` submodule pins a commit from the
[`vtr` branch](https://github.com/ripopov/verilator/tree/vtr), based on
Verilator 5.050. It includes a VTR trace backend next to the existing `--trace-vcd`, `--trace-fst` and `--trace-saif` formats. It is a
plain port of Verilator's FST backend (`verilated_fst_c.{h,cpp}`) onto the VTR
C API (`crates/vtr-capi/include/vtr.h`), so a model built with `--trace-vtr`
runs exactly the same generated trace code as with `--trace-fst`; only the
sink differs.

Build prerequisites include a C++ compiler, make, autoconf, flex, bison, Perl,
and help2man.

```sh
git submodule update --init ext/verilator
integrations/verilator/build.sh bench/build/verilator/install   # build and install pinned source
cargo build --release -p vtr-capi                                # libvtr.a / libvtr.so + vtr.h
verilator --cc --exe --trace-vtr top.sv main.cpp
make -C obj_dir -f Vtop.mk VTR_INCLUDE=$VTR/crates/vtr-capi/include VTR_LIBDIR=$VTR/target/release
```

In the harness, use `VerilatedVtrC` where you would use `VerilatedFstC`
(`#include <verilated_vtr_c.h>`; `verilated_vtr_sc.h` provides the SystemC
wrapper). `verilated.mk` adds `-I$(VTR_INCLUDE)`, `-L$(VTR_LIBDIR)` and
`-lvtr` when `VM_TRACE_VTR=1`; with a static `libvtr.a` no runtime path is
needed. CMake users get `TRACE_VTR` on `verilate()`.

`build.sh` configures Verilator with clang++ when one is installed (set
`CXX` to override): the host compiler is inherited by every model through
`verilated.mk`, and on the benchmark's C910 model gcc generates code that
runs 3x slower than clang's from identical sources unless PGO is used
(`docs/BENCHMARKS.md`, host compiler section).

What the VTR branch adds:

* `src/V3Options.{h,cpp}`: the `--trace-vtr` switch, class base
  `VerilatedVtr`, runtime source `verilated_vtr_c.cpp`; the one-format-only
  check now includes VTR.
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
* `docs/guide/*.rst`: option documentation.

The benchmark suite (`bench/run.py`) builds this Verilator into
`bench/build/verilator/` and uses it for the simulator-integrated
measurements (`docs/BENCHMARKS.md`).

Build products live in `<prefix>/../build` by default; pass a second argument to
select another build directory, and set `JOBS` to control build concurrency.
The script uses the checked-out submodule revision and performs no network clone
or patch application. To reproduce a VTR checkout, use `git submodule update
--init ext/verilator`; `--remote` intentionally selects a newer branch revision.

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
