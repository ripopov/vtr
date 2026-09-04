# State of the art review (September 2026): what could still improve VTR

A survey of 2025-2026 work on trace, time-series and columnar compression,
checked against measurements on the benchmark files of
`docs/BENCHMARK_RESULTS.md`. The question was: which published ideas would
improve VTR's size *and* keep or improve its write and read speed. Ideas that
trade speed for size are listed only to record why they are rejected.

Everything in section 2 was measured on this machine with throw-away tools
(a redundancy scanner and a transform trial over the multi-bit value streams,
both built on the public `vtr` reader API) against the files in
`bench/results/latest/`.

## 1. Summary

* **The biggest remaining cost in the simulator-integrated numbers is not
  VTR's.** On C910, tracing adds 44 s with VTR; the VTR writer accounts for
  9 s of that (replay of the same changes), Verilator's generated
  change-detection and emit code for the other ~35 s (the FST backend shows
  the same split: 52 s = 15 s libfstwriter + ~37 s Verilator). Verilator
  issue #5379 attributes this to instruction-cache misses across thousands
  of per-module trace functions; PR #5806 (draft) splits tracing across
  `--threads` workers into one file per thread, 2.04x geomean faster. A
  VTR-side change cannot recover this cost; a Verilator-side one could.
* **Inside VTR's writer the encoder thread has become the critical path on
  large designs.** On C910 the simulator-side append costs 7.25 s (none/
  background), the encoder 4.75 s of column assembly plus 2.5 s of zstd; the
  caller waits ~1.7 s on it (9.0 s wall). Splitting the encoder over two
  threads (assembly on one, compression on another, or two block halves)
  would bring C910 to ~7.5 s (-17%) without touching the format.
* **Size: one transform is missing from the pool.** Trial-compressing the
  multi-bit value streams of C910 (2-state, 37,497 chunks of 64 KiB) with
  the current four candidates against seven more shows that per-chunk
  **dictionary coding** (≤256 distinct values, 1-byte codes, then zstd) wins
  on 10,414 chunks and takes the best-of-pool total from 159.0 MB to
  148.2 MB (-6.8% on those streams, about -4% of the file); every other
  candidate (bit-plane transpose, frame-of-reference, XOR delta, mask delta)
  is within 0.3%. This is the same conclusion FastLanes (VLDB 2025) reached
  on relational data (DICT is the one operator worth +42% ratio there) and
  dictionary decoding is faster than undoing a delta.
* **Signal-relationship coding (delayed copies, inverted signals, shared
  time columns) has little headroom on Verilator traces.** Whole-file scans:
  byte-identical copies 16% of C910's changes (already removed by block
  aliasing), one-cycle-delayed copies 1.0%, inverted 1-bit copies 0.7%,
  identical value sequences at other times 3.4%, identical change-time sets
  8.8%. The 2025 ISEDA paper on "signal function relationships" targets
  exactly these; on this data they are worth at most a few percent and each
  costs a write-time hash pass.
* **Nothing in the 2025-2026 codec literature displaces zstd for this
  data.** Hishida et al. (VLDB 2025) rank zstd second only to gzip on ratio
  among general and specialised time-series codecs and among the fastest;
  the XOR family (Gorilla, Chimp, Elf, DeXOR 2026) loses an order of
  magnitude of decode speed for a few percent. zstd 1.5.7 (the version the
  crate already links) is the current release and its small-block speed-ups
  (+20% at 32 KiB) are already in the numbers. OpenZL (Meta, October 2025)
  is the interesting new codec: it is a graph of the transforms VTR already
  applies by hand (field split, transpose, delta, tokenise) in front of FSE
  and Huffman; its gains over zstd on structured numeric data (2.06x vs
  1.31x on the SAO catalogue, at higher speed) come from replacing LZ with
  transform-plus-entropy-coding, which is the one thing VTR does not try.

## 2. Measurements behind the conclusions

### 2.1 Write path breakdown (C910, 596.6 M changes, replay through the Rust writer)

| variant | wall | cpu |
|---|---:|---:|
| zstd, background encoder (default) | 8.99 s | 14.2 s |
| zstd, inline | 13.73 s | 13.7 s |
| none, background | 7.25 s | 12.0 s |
| none, inline | 11.24 s | 11.2 s |
| lz4, background | 8.22 s | 13.4 s |

Reading: caller-side append = 7.25 s (12 ns/change; record log push, dedup
compare, epoch check); encoder assembly (counting sort, column encoding,
alias hashing, frames) = 11.24 - 7.25 = 4.0-4.75 s; zstd-3 = 2.5 s; the
default wall (9.0 s) exceeds the caller's own time by 1.7 s because the
single encoder thread (7.2 s of work) is slower than the caller and the
last block is compressed after the loop ends. Both halves are ~7 s, so a
single-threaded improvement to either one alone is worth little on this
workload: the encoder must be parallelised (or made cheaper) *and* the
caller path kept as is.

For the simulator-integrated run the same writer costs 9-10 s of the 44 s
trace overhead (`docs/BENCHMARK_RESULTS.md`, simulator-integrated table).

### 2.2 Read path: where VTR is weakest

The smallest margins are `load 1000 signals` (1.2-3.6x over wellen; slower
than GTKWave's C reader on rsa256_long and c910_coremark). That query opens
the file fresh and loads up to 1000 signals in one call. Two facts from
timing the public API on the whole file:

| file | load all signals | stream all changes |
|---|---:|---:|
| rsa256_long (replay; signals declared 4-state) | 39.7 ns/change | 18.5 ns/change |
| rsa256_long_sim (Verilator; 2-state) | 25.3 ns/change | 18.7 ns/change |
| scr1_axi | 9.3 ns/change | 7.8 ns/change |

Materialising a signal declared 4-state whose changes were stored compact
(2-state) widens every value to the declared packing (`signal::widen`) and
costs 1.6x the 2-state load on the same data. Returning data in the
narrowest packing actually present (with the packing reported in
`SignalData`) would remove that cost for the FST-replayed workloads; the
Verilator-written files do not pay it. This is a reader/API change only.

### 2.3 Redundancy scan (whole file, per signal, counted for the second and later copy)

| workload | changes | exact copy | delayed copy | inverted 1-bit | same values, other times | same times, other values |
|---|---:|---:|---:|---:|---:|---:|
| scr1_axi | 21.0 M | 27.5% | 1.2% | 2.1% | 4.7% | 4.4% |
| scr1_x8 | 167.8 M | 50.8% | 0.0% | 0.3% | - | 40.6% |
| c910_coremark (Verilator) | 596.6 M | 16.4% | 1.0% | 0.7% | 3.4% | 8.8% |
| rsa256_long | 16.2 M | 0.5% | 0.1% | 0.0% | 0.1% | 24.7% |
| long_sparse, many_active, wide_bus | | 0% | 0% | 0% | 0% | 0% |

"Exact copy" is what block-level dynamic aliasing already removes (the
scan is whole-file, the writer works per block, so the writer catches at
least this much). "Delayed copy" allows any constant time shift and one
extra leading entry (reset values); "same times" means the header (time
delta) stream is identical while values differ, which zstd already exploits
when the two signals fall in the same 64 KiB run.

Value statistics of C910's 2-state multi-bit signals (306 M changes of
21,245 signals up to 64 bits): 73% of consecutive deltas fit in one byte,
23% leave the upper bytes unchanged, 6.5% repeat the previous delta.
Counter-like behaviour is common enough for the delta transform to keep
winning where it wins today, not common enough for delta-of-delta.

### 2.4 Transform trial on multi-bit value streams (C910, Verilator file, 2-state)

Per signal, values cut into 64 KiB chunks (37,497 chunks, 1.95 GB raw),
each zstd-3 compressed under every candidate; totals in bytes and the
number of chunks a candidate wins outright.

| candidate | total | wins |
|---|---:|---:|
| none | 175.8 MB | 14,857 |
| byte shuffle | 190.6 MB | 4,291 |
| delta | 209.0 MB | 1,942 |
| delta + shuffle | 248.4 MB | 157 |
| XOR delta | 198.3 MB | 2,349 |
| XOR delta + shuffle | 208.1 MB | 1,590 |
| bit-plane transpose | 317.6 MB | 960 |
| frame-of-reference + shuffle | 191.1 MB | 654 |
| frame-of-reference + bit-plane | 320.9 MB | 34 |
| dictionary (≤256 values, 1-byte codes) | 53.0 MB on the 10,663 chunks where applicable | 10,414 |
| dictionary + delta of codes | 62.2 MB (same chunks) | 249 |
| **best of the current four** | **159.0 MB** | |
| **best of all eleven** | **148.2 MB (-6.8%)** | |

By entry width the gain concentrates on 2-6 byte values (5-25%), 12-byte
(30%) and 23-byte (17%, the largest stream in the file); wide high-entropy
values gain nothing. On rsa256_long_sim the whole pool gains 0.2%: the
trial mechanism correctly keeps plain values there.

## 3. Sources and what each contributes

### Waveform-specific

* **Gao, Xie, Yu, "Efficient and Effective Digital Waveform Compression for
  Large-scale Logic Simulation", GLSVLSI 2023** (Tsinghua): per-value-class
  encoding with lookup tables, 402x average (up to 1561x) over VCD text.
  Ratios against VCD text are not comparable with VTR's (VTR is 63% of FST
  on C910, and FST is already ~50x below VCD); the paper's mechanism is a
  variant of FST's per-signal chains.
* **He, Chen, "A Lossless Compression Method for VCD Files Based on Signal
  Function Relationships, Value Prediction and Bit-Plane Reorganization",
  ISEDA 2025** (abstract only accessible): predicts signals from other
  signals, predicts values from history, reorganises bits into planes. Each
  of the three was measured here: relationships (section 2.3) are worth
  ≤5% of changes on Verilator traces; value prediction is VTR's delta
  transform; bit-plane reorganisation loses to plain zstd on every C910
  chunk class (section 2.4) because zstd's match finder already sees the
  byte-level repetition that planes expose, and planes destroy it.
* **Verilator issue #5379, "Improve trace performance on huge designs"**
  (open, discussion): TraceDump is 70-90% of the main thread on huge
  designs with 50-100% instruction-cache miss rates in the per-module
  `trace_chg` functions; proposes centralised change detection in the
  emitter. **PR #5806** (draft): one FST file per `--threads` worker,
  2.04x geomean speed-up, 1.13x smaller dumps, blocked on viewer support
  for multi-file traces. **PR #6992 / release 5.050**: Verilator's FST
  backend now uses libfstwriter ("2x faster than fstapi"); multi-threaded
  FST tracing was removed (#7443) as incompatible with it. These frame the
  44 s: the writer is 20% of the trace cost, Verilator's generated code 80%.
* **Synopsys, "2x faster waveform dumping in Verdi with VCS"**: dynamic
  aliasing of signals that share activity "90% of the time" (1.5x faster,
  5x smaller FSDB), multithreaded dumping, fewer UVM callbacks. VTR's
  block-level aliasing is the same idea at 16M-record granularity; the
  "90% of the time" phrasing means their aliases are per time range, which
  VTR's per-block aliases also are.
* **Surfer (CAV 2025), wellen, vaporview**: viewers; wellen's FST-inspired
  in-memory layout is what VTR's `load_signals` output already mirrors. No
  new on-disk format from the viewer side in 2025-2026.

### Columnar and time-series storage

* **Afroozeh, Boncz, "The FastLanes File Format", VLDB 2025**: no LZ at
  all; cascades of light-weight operators (FFOR, DELTA in the transposed
  layout, DICT, RLE, ALP, FSST, CONSTANT, EQUALITY, one-to-one maps) chosen
  by trial-encoding three 1024-value vectors per 64K-row group. Decode
  43x faster than Parquet+zstd, files 2% smaller on Public_BI but 15%
  larger on TPC-H; ablation: DICT +42% ratio and +44% decode speed, DELTA
  +6% ratio for -2% speed, EQUALITY +4.7%. Takeaways used here: dictionary
  is the operator to add (confirmed in 2.4); sample three places per run
  rather than the first 1 KiB (cheap, low risk); everything else in the
  cascade either exists in VTR (delta, equality = aliases, constant =
  dedup) or loses under zstd on this data.
* **Hishida et al., "Beyond Compression: A Comprehensive Evaluation of
  Lossless Floating-Point Compression", VLDB 2025**: single Rust
  implementation of Gorilla, Chimp, Chimp128, Elf, Sprintz, Buff, ALP,
  gzip, snappy, zstd. Ratio: gzip > zstd > ALP > Elf; throughput: ALP
  highest, zstd and snappy next, XOR coders lowest by an order of magnitude
  (sequential bit dependencies). Query throughput tracks decompression
  throughput. Confirms VTR's byte-level transforms + zstd over bit-level
  XOR coders; applies to VTR's reals, which are rare in RTL traces.
* **Pace et al., "Lance: Efficient Random Access in Columnar Storage
  through Adaptive Structural Encodings", 2025**: 4-8 KiB compressed
  chunks cost nothing on scans and are optimal for random access on NVMe;
  inline page statistics are "extremely detrimental"; readers lose up to
  2x by alternating I/O and decode instead of overlapping them. VTR's 64 KiB
  raw runs compress to 4-20 KiB, in the recommended range; VTR keeps no
  inline statistics; the reader is memory-mapped and does not prefetch,
  which matters only on cold cache and is not what the benchmark measures.
* **Xie et al., "TRACE: Unlocking Effective CXL Bandwidth via Lossless
  Compression and Precision Scaling", 2025**: bit-plane layout plus
  per-channel base-delta before LZ4/zstd on BF16 tensors, 1.33x -> 1.88x
  over zstd on raw. Measured here as "bit-plane" and "FOR + bit-plane":
  loses on RTL values (section 2.4). Floating-point exponent planes are
  low-entropy in a way that bus values are not.
* **Tang et al., "LogLite: Lightweight Plug-and-Play Streaming Log
  Compression", 2025**: XOR-P (emit the original byte where it differs and
  zero where it matches) beat true XOR residuals by 21% because originals
  keep their distribution for the LZ stage; byte-aligned RLE beat
  bit-aligned by 29%. The first argues for the mask-delta variant tried as
  "XOR delta" here (wins 2,349 chunks but never beats dictionary or plain);
  the second matches VTR's existing choice of byte streams and varints over
  bit-packing in front of zstd.
* **Meta, OpenZL (October 2025)**: format-aware graph of transforms
  (struct split, transpose, delta, tokenise, dispatch) resolved per frame
  and embedded in it, with a trainer that searches transform graphs and
  clusters similar fields; final stage FSE/Huffman, not LZ. 2.06x at
  340 MB/s compress / 1.2 GB/s decompress on SAO vs zstd-3 1.31x at
  220 MB/s / 850 MB/s. VTR performs the same decomposition (header and
  value streams, shuffle, delta, dictionary) but hands the result to zstd;
  the missing experiment is entropy-only coding (FSE) of transformed
  streams whose sample shows no LZ matches, which would be faster on both
  sides and possibly smaller on high-entropy wide values (98% of FST on
  rsa256 today).
* **Blosc2 bytedelta (2023-2025)**: delta of bytes *after* shuffle, 25%
  better than bitshuffle on a climate dataset at >20 GB/s. Equivalent to
  VTR's delta + shuffle candidate up to carry handling; not a new option.
* **FSST / OptFSST (2025), Nimble, Vortex**: string dictionaries and
  cascading encoding frameworks; relevant only to the string table and
  variable-length signals, which are negligible in the benchmark files.
* **DeXOR (VLDB 2026), Chimp, Elf, ALP**: floating-point coders; VTR's
  reals are stored as 8-byte values under the run transform, and the
  workloads contain none. ALP's integer-scaling trick would be the choice
  if real-valued traces (analogue, SystemC AMS) ever matter.
* **CLP, LogShrink, Mint (2024-2025)**: log compression by template and
  variable separation, column stores of variables; VTR's transaction
  blocks already store attributes column-wise with interned keys (18-34%
  of FTR-LZ4), and the log-template idea does not map onto signal values.

## 4. Ranked ideas

Each entry says what the evidence is, what it would change, and the
expected effect on the three axes (size / write / read).

### Do (measured headroom, no trade-off)

1. **Dictionary transform for value streams** (implemented as transform 4,
   see the outcome below) (`xform` 4: sorted
   dictionary of ≤256 distinct entries + 1-byte codes, then zstd; decided
   by the existing sample trial). Evidence: section 2.4, -6.8% on C910
   multi-bit value streams (~-4% of the file), 0% where it does not apply.
   Write: one hash pass on the sample and, when chosen, on the run
   (cheaper than delta with carries). Read: a table lookup per value, faster
   than undoing delta. `docs/RATIONALE.md` records dictionary coding as
   rejected because zstd finds repeats within a run; the C910 trial says
   otherwise for 2-6 byte and 12-23 byte values, where a one-byte code
   stream compresses 3x better than the matches zstd finds in 2-23 byte
   entries. Caveat: the trial compresses one signal per chunk while real
   runs mix up to 64 signals, which gives zstd more context; the writer
   experiment must confirm the gain on whole files before the format
   gains a transform code.
2. **Second encoder thread.** Evidence: section 2.1, encoder 7.2 s versus
   caller 7.25 s on C910, 1.7 s of waiting; on wide-value workloads the
   compression share is larger. Pipeline assembly (sort + column encode)
   and compression (transform trial + zstd) on separate threads, or encode
   two blocks concurrently. Size unchanged; write -15-20% wall on C910,
   more on wide_bus/rsa256; read unchanged. CPU time rises slightly (two
   threads' cache footprint). Inline mode is unaffected.
3. **Return loaded values in the packing actually stored.** Evidence:
   section 2.2, 1.6x load cost from widening compact 2-state entries to a
   declared 4-state packing. Add the packing to `SignalData` and widen
   only on request. Size and write unchanged; `load N signals` on the
   FST-replayed workloads (four of eight) 1.3-1.6x faster.
4. **Three-point sampling for the transform trial** (FastLanes). Take the
   1 KiB sample from the start, middle and end of the run instead of the
   start. Cost: two more trial compressions of 1 KiB per run (~2% of
   encoder time); gain: correct choices on runs whose behaviour changes
   mid-run. Expected small (≤1%) but strictly non-negative on size.

   *Outcome (implemented 2026-09-04, full suite rerun):* the sample trial
   over-predicts the dictionary's gain (32-entry segments favour codes over
   the long-range matches zstd finds in plain or shuffled values): at a 0%
   margin it lost on 625 of 839 runs of scr1_x8 (+0.9% file). With a 20%
   margin, an order-0 pre-estimate on the sample's codes (so runs that
   cannot win pay no extra trial) and whole-column hashing only after that
   pre-estimate passes, the suite shows C910 -1.1% (245.94 -> 243.13 MiB;
   Verilator-written file 263.2 -> 260.3 MB), scr1_axi -0.5%, scr1_x8
   -0.1%, every other workload byte-identical; write times within noise
   (-4% to +2% across the 24 writer measurements), reads unchanged,
   parity and the VCD information check clean. The chunk-level -6.8%
   headroom does not survive the run-level decision: real runs mix up to 64
   signals, and the plain sample already compresses most low-cardinality
   columns well. The remaining gap is a better decision, not a better
   coder; compressing both candidates in full for marginal runs would
   recover it at ~5% encoder time.

### Try (plausible, needs an experiment)

5. **Entropy-only coding (FSE or Huffman) for runs whose sample shows no LZ
   gain** (OpenZL's final stage). Today such runs go to LZ4 and stay
   nearly raw (rsa256 98% of FST, wide_bus 97%). Order-0 coding of shuffled
   byte lanes could take 5-10% off high-entropy wide values at higher speed
   than zstd-1. Risk: order-0 on true random data gains nothing and the
   probe must stay cheap; FSE tables cost 256 bytes per lane per run.
6. **Header-stream sharing across signals with identical change times**
   (FastLanes EQUALITY on the time column). Evidence: 8.8% of C910's and
   40% of scr1_x8's changes belong to signals whose change-time sets equal
   another signal's; zstd already removes most of that when the signals
   share a run. Worth measuring the compressed header share first (the
   writer can report header vs value bytes per run); expected ≤2% size,
   read-neutral.
7. **Aliases modulo the first entry** (delayed and inverted copies): 1-2%
   of changes on real RTL. Only worth it if it fits the existing alias
   hash (hash the streams from the second entry and store the first entry
   in the alias record); otherwise skip.
8. **Verilator side: reduce the generated trace code's cost.** Not a VTR
   change, but the only route to a materially lower simulator-integrated
   number (44 s -> the writer's 9 s is the floor for the current Verilator
   code). Options in order of realism: (a) `--trace-vtr` with PR #5806's
   per-thread files, since VTR's container can carry several signal
   partitions and a reader merge is straightforward; (b) chunked
   change-detection loops in `V3EmitCImp` as issue #5379 suggests; (c)
   PGO of the traced model (the compiler study shows PGO alone gives 5x on
   the untraced model with gcc and 1.5x with clang; the traced model was not
   measured and should be).

### Rejected by measurement or by the literature

* Bit-plane reorganisation (ISEDA 2025, TRACE): +80% on C910 value streams
  versus plain zstd; wins only 960 of 37,497 chunks and never by enough to
  matter.
* Frame-of-reference before shuffle: 0-0.3% better than shuffle alone;
  not worth a fifth candidate.
* XOR / mask delta: wins 2,349 chunks but the pool total with it is
  within 0.1% of the pool without; redundant with delta + dictionary.
* Delta-of-delta / counter models: 6.5% of consecutive deltas repeat on
  C910; too rare.
* XOR-based streaming coders (Gorilla, Chimp, Elf, DeXOR) for anything but
  reals: order of magnitude slower decode for a few percent (Hishida 2025).
* Replacing zstd with LZ4 for speed: measured, +69% size on C910 for -9%
  write wall; with a second encoder thread the zstd wall cost disappears
  anyway.
* Neural or learned compression (2026 image/video work): no lossless
  waveform result exists and decode speed would fall by orders of
  magnitude.
* Smaller runs for random access (Lance): VTR's runs already compress to
  the 4-20 KiB range Lance recommends, and 1M-record blocks were measured
  at +58% size (`docs/RATIONALE.md`).

## 5. Reproducing the measurements

The throw-away tools are not part of the repository; they use only the
public reader API (`Reader::open`, `load_signals`, `for_each_change`) and
the `zstd` crate, so re-creating them is a few hundred lines:

* redundancy scan: per signal, hash (times, values), (time deltas from the
  second entry, values), the same with the first entry dropped, values
  only, times only; count changes of every signal whose key was already
  seen;
* transform trial: per 2-state multi-bit signal, cut the value stream into
  64 KiB chunks, apply each candidate transform, `zstd::bulk::compress`
  level 3, sum the minima;
* write breakdown: `vtr-bench write <rpl> <out> [--codec none|lz4]
  [--no-background]` on `bench/workloads/gen/c910_coremark.rpl`.
