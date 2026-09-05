# Benchmark methodology

Results are in `docs/BENCHMARK_RESULTS.md` (generated) with raw numbers in
`bench/results/latest/results.json`. This document describes what is
measured, how, and how to reproduce it.

## Reproducing

```sh
git submodule update --init          # libfstwriter (contains GTKWave's fstapi.c), LWTR4SC, Konata, wavepeek, pulp-c910
python3 bench/run.py all             # full set (~1 hour, ~16 GB RAM); the first run also builds Verilator and the C910 models
python3 bench/run.py all --scale small
python3 bench/run.py report          # re-render the Markdown from results.json
python3 bench/run.py compilers       # host-compiler study on the C910 model (~1.5 hours)
```

`bench/run.py` builds the Rust crates (`cargo build --release`), the C/C++
harnesses (`bench/cpp`, CMake, needs zlib and liblz4 development files),
Verilator 5.050 with the `--trace-vtr` backend (`integrations/verilator`,
a shallow clone of the pinned upstream tag into `bench/build/verilator/`;
needs autoconf, flex, bison), prepares the workloads under
`bench/workloads/gen/`, runs every measurement best-of-N (default 3;
simulator runs best-of-2) and renders the report. Apart from the Verilator
source, nothing is downloaded: every input is generated from the
repository or taken from the submodules. The C910 workload needs a
`riscv64-unknown-elf-gcc` for the CoreMark image.

## Competitors

| label | what |
|---|---|
| fstapi LZ4 / zlib | GTKWave's original `fstapi.c` writer (vendored in `ext/libfstwriter/integration_test/verilator_share/gtkwave/`), pack type LZ4 or zlib (zlib is GTKWave's default) |
| libfstwriter LZ4 | `ext/libfstwriter` C++14 re-implementation ("2x faster than fstapi"), its only pack type |
| wellen (FST) | the `wellen` crate (with `fst-reader`) that wavepeek uses; all read timings use its public API the way wavepeek does |
| fstapi reader | `fstReaderOpen`, `fstReaderIterateHier`, `fstReaderGetValueFromHandleAtTime`, `fstReaderIterBlocks2` |
| FTR (LZ4 / raw) | `ext/LWTR4SC/src/ftr/ftr_writer.h`, `ftr_writer<true>` (LZ4) and `ftr_writer<false>` |
| uncompressed variants | `fstapi none` = pack type FASTLZ in the vendored build, where fastlz is compiled out so every value chain is stored raw (time tables and frames stay zlib-packed); `libfstwriter none` = its `NO_COMPRESSION` mode; `VTR none` = codec none for every blob. Reads of the uncompressed pair use wellen on the libfstwriter file. |
| VTR Rust / C API | this repository, default options (zstd level 3, 256-signal groups, 64 KiB runs, 16M-record blocks, 512K-record chunks, background thread on); `inline` variants disable the background thread so the encoder runs on the caller's thread |
| Verilator FST / **VTR** | the Verilated model writing its own trace: Verilator 5.050's built-in FST backend (bundles libfstwriter, LZ4) versus the `--trace-vtr` backend of `integrations/verilator` (VTR C API, default options); both run the same generated trace code |

## Workloads

Signal workloads are *replays*: the writer-neutral change stream
(`bench/workloads/gen/*.rpl`, format in `crates/vtr-bench/src/replay.rs`)
is decoded into memory once, and every writer consumes exactly the same
sequence of (time, signal, value) with the API call that suits its
interface (`fstWriterEmitValueChange64/Vec32`, ASCII strings for x/z
values, `emitValueChange(handle, words)`, `vtr_writer_emit_u64/packed`).
Timing covers hierarchy declaration, the whole replay loop and close.

| workload | origin | shape |
|---|---|---|
| `scr1_axi` | `ext/wavepeek/web/playground/assets/scr1_axi.fst`, an SCR1 RISC-V core with AXI testbench traced by Verilator (real RTL) | 1.4k signals, 21M changes, 376k time steps |
| `rsa256` | RSA-256 Montgomery multiplier from `ext/libfstwriter` integration tests, driven with continuous pseudo-random operands by `bench/workloads/rsa256/RSA_bench_tb.sv`, Verilated by `bench/workloads/rsa256/build.sh`, 200k cycles (real RTL, 256-bit datapaths) | 113 signals, 1.6M changes, mostly wide vectors |
| `rsa256_long` | same design and driver, 2M cycles (long simulation) | 16M changes, 4M time steps |
| `c910_coremark` | pulp-c910 (`ext/pulp-c910`: T-Head's openC910, a 3-wide superscalar out-of-order RV64GC core with L1 caches, MMU and AXI SoC, ~530 modules) running one CoreMark iteration under Verilator, built by `bench/workloads/c910/Makefile` (cacheable main memory, one width fix in `ct_lsu_ctrl.v`, one CoreMark iteration instead of the hard-coded two); every signal of the design is dumped after each clock edge (real RTL, large design) | see the report: hundreds of thousands of signals, ~240k cycles |
| `scr1_x8` | 8 copies of `scr1_axi` under separate top scopes, multi-bit values XOR-perturbed per copy so copies are not byte-identical; models a multi-core SoC trace | 11.6k signals, 168M changes |
| `long_sparse` | synthetic: 20k signals in 200 modules, 500k time steps, ~20 signals change per step | long run, few active signals |
| `many_active` | synthetic: 200k signals, 1000 cycles, 30% change per cycle | short run, many active signals |
| `wide_bus` | synthetic: 64 buses of 256..2048 bits with valid strobes, 10k cycles | wide buses |

Synthetic generators are deterministic (`crates/vtr-bench/src/replay.rs`,
fixed seeds). They exist to cover shapes the two real designs do not;
claims are made per workload and the report labels each one.

Transaction workloads:

| workload | origin | shape |
|---|---|---|
| `tlm_1m` | synthetic TLM traffic: 4 CPUs issue loads/stores that become NoC packets that become slave accesses; every level has 3..5 typed attributes; `parent_of` and `pred` relations (`vtr-bench gen-tlm`) | 3M transactions, 14M attributes, 3M relations |
| `ooo_1m` | synthetic Kanata log of a 4-wide out-of-order core: F/Dc/Rn/Ds/Is/X/Cm/Rt stages, issue stalls on lane 1, 1..2 wakeup dependencies per op, ~14% branches with 10% mispredict flushes, labels with PC and disassembly (`vtr-bench gen-kanata`) | 1M instructions, ~8M stages, ~9M relations |
| `kanata_sample2` | `ext/Konata/docs/kanata-sample-2.log.gz` (real Konata sample) | 4k instructions |

For TLM the FTR harness (`bench/cpp/ftr_write.cpp`) and the VTR driver
consume the same replay (`.txr`). For Kanata both sides parse the same
text log: `bench/cpp/konata_ftr.cpp` maps stages to child transactions
with `parent_of` relations (FTR has no stages), VTR uses native stages;
timings include parsing on both sides.

## Measurements

Writers: wall-clock seconds and process CPU seconds (user+system, all
threads) of declare + replay + close, best of N (a single run for replays
above 200M changes, where one FST write takes minutes); output size in
bytes.
The VTR "cpu" column therefore includes the background encoder; the wall
column is what a simulator experiences.

Readers (`crates/vtr-bench/src/read.rs`), each best of 3, fresh process
model (a new open per measurement, like a one-shot CLI such as wavepeek):

| query | wellen | VTR |
|---|---|---|
| open | `wellen::simple::read` | `Reader::open` |
| hierarchy walk | all vars with `full_name` | all var nodes with `full_path` |
| load N signals (1/10/100/1000, random) | `load_signals` | `load_signals` |
| value at time, 10 signals x 100 times | load the 10 signals, then `get_offset`/`get_value_at` | `value_at` (no load) |
| changes of one signal in a 1% window | load + filter by time-table index | `changes(sig, t0, t1)` |
| condition search: posedge clk && bus == v over the whole run | load both, iterate | `load_signals` both, iterate |
| stream every change | `fst-reader::read_signals` (all) | `for_each_change` |

The existing load-N plan samples **with replacement**. Thus "load 1000" means
1000 requests, potentially for fewer unique signals (especially on RSA's 113
signals). VTR shares repeated histories within a load call. To separate bulk
loading from repeated-request handling, use:

```sh
vtr-bench load trace.vtr unique 1000     # up to 1000 distinct signals
vtr-bench load trace.vtr requests 1000   # 1000 requests sampled with replacement
```

Both modes report request and unique-signal counts, logical change counts, and
best-of-3 wall time including open, load, and destruction. The plan is generated
outside the timer with seed 42 by default (`--seed` overrides it). Unique
sampling uses a bounded number of draws, including when requesting every signal
or using seed zero; requests mode preserves repeated IDs. For an A/B
comparison, compile the same `load.rs` harness and command dispatch against both
revisions, keep the baseline library unchanged, and use the same trace file.
Compare writer size/time separately with the standard `write` command.

The same signal/time plan (seeded) is exported to the C harness so that
`fstapi` numbers use identical queries. Results are cross-checked: value
strings and change counts must match between wellen and VTR (`parity`).
Every reader process runs with glibc's dynamic mmap threshold disabled
(`MALLOC_MMAP_THRESHOLD_=32 MiB`, `MALLOC_TRIM_THRESHOLD_=512 MiB`):
otherwise the best-of-3 time of a load depends on whether an earlier
iteration happened to raise the threshold (freed buffers reused without
page faults) or not, which varies with allocation sizes rather than with
the work done. The setting applies equally to wellen, fstapi and VTR.
The FST input for reads is the file written by `fstapi` with zlib (the
GTKWave default); for the two simulator-produced workloads the report
also includes reads of the original simulator file.

Simulator-integrated tracing (`bench/workloads/rsa256/tb.cpp`,
`bench/workloads/c910/tb/sim_main.cpp`): the same Verilated design is
built three times, without tracing, with `--trace-fst` and with
`--trace-vtr`, and each executable is run as a whole process (best of N)
with the full design dumped after every clock edge. Reported: wall and
CPU seconds of the whole run including closing the file, the trace cost
(wall minus the untraced run) and the file size. The two simulator-written
files are then read with the same query set as above (`vs_sim` in the
results), which also checks that the VTR backend recorded exactly what
Verilator's FST backend did.

Information check (`vcd_check` in `bench/run.py`, last step of every
workload that has a simulator FST): the FST is converted to VCD with
GTKWave's `fst2vcd` when it is installed (an implementation independent
of everything in this repository; `vtr fst-to-vcd` otherwise), the
replay-written VTR and the simulator-written VTR are converted with
`vtr2vcd`, and `vtr vcd-compare` counts the value changes of the VCDs in
total and per signal. The counts must be identical: this is the proof
that the size and speed comparisons are made on the same information
and that nothing is dropped on the way into VTR.

Host-compiler study (`bench/compilers.py`, rendered as its own section):
the Verilated sources of the untraced C910 model are compiled with the
latest installed gcc and clang at `-O2`, `-O3`, `-O3 -march=native`, each
with and without profile-guided optimisation (trained on the first 60k
cycles of the same run) and with link-time optimisation, plus older
compiler versions and a few single-optimisation probes for gcc; every
binary runs the same CoreMark iteration pinned to one performance core,
best of N. The table also lists text size and the share of instructions
with a memory operand, which is where the two compilers differ most on
this code.

Transaction navigation (`vtr-bench tx-read`): open, scan all
transactions, 1000 random lookups by id, 1000 relation queries (from and
to), a 1% time-window query. FTR has no reader library beyond a Python
dumper, so there is no FTR column.

## Environment

The report records CPU model, thread count, memory, kernel, compiler and
Verilator versions and the date. Numbers below one millisecond are noisy;
run-to-run variation on this machine is within 5% for everything above
10 ms.

## Caveats

* Replays exercise the writers' APIs, not Verilator's tracing glue; the
  extra work a simulator does around any writer (value comparison,
  callback dispatch) is the same for all writers and is not measured.
  The simulator-integrated rows measure exactly that end to end, for the
  two writers Verilator can drive (its own FST backend and VTR).
* The Verilated models are single-threaded and the traced ones are built
  the same way as the untraced one (`-O3`, host `-O2`, clang when
  available: g++ 15 makes the C910 model 3x slower); Verilator's
  offloaded/parallel tracing is not used by either backend.
* libfstwriter has no variable-length values and no blackout; the
  harness skips such changes for it (counted as `skipped`, 0 in all
  workloads here).
* `fstapi` numbers use the vendored sources compiled with `-O2`, LZ4 from
  the system, and without `FST_WRITER_PARALLEL`, which the vendored build
  does not enable.
* Synthetic workloads are labelled synthetic; the summary claims are
  supported by the real RTL traces and by the transaction workloads, with
  the synthetic ones showing the extreme shapes.

## Discussion of the results

The numbers in `docs/BENCHMARK_RESULTS.md` support the three claims of
the requirements on every workload of the suite, with the following
qualifications, which are the honest boundaries of the claims:

* **Size.** VTR is smaller than the smallest FST variant (GTKWave's
  default zlib packing) on all eight signal workloads: 51% of FST-zlib on
  the SCR1 core, 47% on its 8-copy replica, 63% on the openC910 CoreMark
  trace (246 MiB against 389 MiB zlib and 501 MiB LZ4 for 597M changes),
  61-69% on the synthetic designs, and 97-98% on the high-entropy RSA-256
  datapath and wide-bus workloads, where both formats sit near the
  entropy floor of the values themselves and the writer's transform trial
  correctly keeps plain values. Against LZ4-packed FST (what libfstwriter
  and Verilator produce) the margins are larger still. Transaction files
  are 3-5x smaller than LZ4-compressed FTR.
* **Write speed.** With the default background encoder VTR is faster than
  the fastest FST writer (libfstwriter) on every signal workload, by
  1.13x (wide buses, where copying 256-2048-bit values dominates both
  writers) to 2.1x (SCR1); 1.67x on the C910 trace (9.0 s against 15.0 s
  for libfstwriter and 63 s for fstapi zlib), and 10-14x faster than
  GTKWave's own `fstapi.c`. The CPU column shows the price: the background thread adds
  10-60% total CPU. The `inline` variants (encoder on the caller thread)
  are also faster than libfstwriter on every workload (1.12-1.48x), so the
  speed does not depend on a spare core; the per-run transform trial and
  the transform pass cost the inline encoder about 2%, paid back by the
  cheaper column-alias hashing and by chunks that grow with the signal
  count. For transactions VTR beats LZ4-compressed FTR by 1.7-1.8x and
  uncompressed FTR by 1.3x; the inline variant is slower than uncompressed
  FTR on the TLM workload, the one configuration where FTR writes faster.
  The 4k-instruction Konata sample is dominated by process start-up on
  both sides (parity).
* **Simulator-integrated.** Inside Verilator, with the whole openC910
  design dumped after every clock edge, the `--trace-vtr` backend adds
  44 s to a 12 s CoreMark simulation where Verilator's built-in FST
  backend (libfstwriter, LZ4) adds 52 s, and writes a file half the size
  (251 MiB against 501 MiB); on the RSA-256 runs the trace cost is 1.4x
  lower and the file 4% smaller. Both backends run the same generated
  trace code (Verilator's change detection dominates the trace cost on
  this design), so the difference is the writer alone; the CPU column
  shows that VTR's background thread costs about 14% extra CPU on the
  C910 run.
  The files written by the two backends read back identically (parity
  in the `vs_sim` rows). The host C++ compiler matters more than the
  Verilator version here (see the host-compiler section below): the same
  sources compiled by gcc run 3x slower than compiled by clang, so the
  suite builds Verilator, and therefore every model, with clang when it
  is available and the report names the compiler used.
* **Host compiler.** On the untraced C910 model, every gcc 15/16 build
  without profile feedback runs the CoreMark iteration in about 36 s,
  whatever the flags (`-O2`, `-O3`, `-march=native`, `-Os`, LTO, Ubuntu's
  hardening off, and the vectorizer, second scheduling pass, PRE/GCSE,
  branch guessing, block and function ordering, if-conversion, inlining
  limits and alignment each switched individually); every clang 19/21/22 build runs
  it in 10.4-12.1 s. With profile-guided optimisation both compilers
  land at about 7 s (clang `-O3 -march=native` PGO: 6.8 s), so PGO is
  worth 5x for gcc and 1.7x even for clang on this code, and the choice
  of compiler stops mattering. Where the plain gcc code goes: Verilator
  turns `case` statements and priority muxes into nested `?:` trees
  thousands of lines long, and gcc emits about 7.5x more instructions
  than clang for them (8,619 against 1,148 for one 18,000-line decoder
  function, at `-O1` as at `-O2`), 21% more code for the whole model;
  with a profile it moves the rarely taken leaves into `.cold` sections
  (0.67 MB of them, against 151 bytes without a profile). Which gcc pass
  expands these trees was not identified: none of the sixteen passes and
  parameters tried changes the count. The benchmark models are built
  without PGO, the way simulator users build them.
* **Same information.** On every workload that comes from a simulator
  (SCR1, both RSA-256 runs, C910 CoreMark) the FST converted by GTKWave's
  `fst2vcd` and the VTR files converted by `vtr2vcd` hold the same number
  of value changes, in total and per signal: 597M changes on the C910
  trace for the FST, the replay-written VTR and the Verilator-written VTR
  alike. The size and speed comparisons are therefore made on identical
  content; VTR drops nothing.
* **Read and navigation.** Against wellen (wavepeek's reader) VTR is
  faster on every query on every workload. On the C910 trace, opening
  takes 8.5 ms against 25 ms, walking the 205k-var hierarchy 1 ms against
  20 ms, loading one signal 7 ms against 80 ms, and streaming all 597M
  changes 6.4 s against 128 s for fst-reader (15 s for GTKWave's C
  reader). Random value queries are 2-5x
  faster, single-signal loads 4-11x, windowed change scans 4-11x,
  streaming every change 2.3-76x, and opening a file 2-800x. The two
  former exceptions on the synthetic 200k-signal `many_active` design are
  gone: opening it takes 5 ms instead of 11 (column-wise hierarchy,
  relative ids) and loading 1000 random signals 29 ms instead of 54
  (runs capped at 64 signals), 1.3x faster than wellen. Streaming SCR1
  takes 0.17 s for 21M changes (0.38 s for fst-reader) with memory
  proportional to the number of signals, not changes.
* **Uncompressed.** With compression switched off on both sides VTR's
  files are within a few percent of raw-chain `fstapi` on the real
  designs (4% larger on the C910 trace, where VTR stores more time-index
  bytes for 67k mostly 1-bit signals) and smaller on the synthetic ones; both FST variants still
  zlib-pack their time tables and frames, so they are not fully raw.
  Uncompressed VTR writes 1.3-2x faster than libfstwriter's
  `NO_COMPRESSION` mode and reads faster on every query shown. The
  comparison isolates the encoding: implied-toggle 1-bit entries, packed
  multi-state vectors, delta-coded time indexes and dynamic aliasing
  versus FST's per-chain layout (value transforms are only applied to
  compressed runs).
* GTKWave's C reader (`fstapi`) is included for reference: its streaming
  path is close to VTR on small files but its random-access path
  (`fstReaderGetValueFromHandleAtTime`) is 15-500x slower than
  `value_at`.

The report's *Summary* section is generated from the same data and lists
every workload where a claim does not hold.

Reproduce a single workload with `python3 bench/run.py run --workloads
scr1_axi`, and only the read tables with `--reads-only`.
