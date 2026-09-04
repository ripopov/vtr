# Benchmark methodology

Results are in `docs/BENCHMARK_RESULTS.md` (generated) with raw numbers in
`bench/results/latest/results.json`. This document describes what is
measured, how, and how to reproduce it.

## Reproducing

```sh
git submodule update --init          # libfstwriter (contains GTKWave's fstapi.c), LWTR4SC, Konata, wavepeek
python3 bench/run.py all             # full set (~10 minutes, ~8 GB RAM); needs verilator for the RSA256 workloads
python3 bench/run.py all --scale small
python3 bench/run.py report          # re-render the Markdown from results.json
```

`bench/run.py` builds the Rust crates (`cargo build --release`), the C/C++
harnesses (`bench/cpp`, CMake, needs zlib and liblz4 development files),
prepares the workloads under `bench/workloads/gen/`, runs every
measurement best-of-N (default 3) and renders the report. Nothing is
downloaded; every input is generated from the repository or taken from
the submodules.

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
threads) of declare + replay + close, best of N; output size in bytes.
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
  default zlib packing) on all seven signal workloads: 51% of FST-zlib on
  the SCR1 core and 47% on its 8-copy replica, 61-69% on the synthetic
  designs, and 97-98% on the high-entropy RSA-256 datapath and wide-bus
  workloads, where both formats sit near the entropy floor of the values
  themselves and the writer's transform trial correctly keeps plain
  values. Against LZ4-packed FST (what libfstwriter and Verilator
  produce) the margins are larger still. Transaction files are 3-5x
  smaller than LZ4-compressed FTR.
* **Write speed.** With the default background encoder VTR is faster than
  the fastest FST writer (libfstwriter) on every signal workload, by
  1.13x (wide buses, where copying 256-2048-bit values dominates both
  writers) to 2.1x (SCR1), and 10-14x faster than GTKWave's own
  `fstapi.c`. The CPU column shows the price: the background thread adds
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
* **Read and navigation.** Against wellen (wavepeek's reader) VTR is
  faster on every query on every workload. Random value queries are 2-5x
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
  designs and smaller on the synthetic ones; both FST variants still
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
