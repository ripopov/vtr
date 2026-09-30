# Crash lab

Measurements behind [the crash-safe VTR design](../../docs/crash-safe-vtr.html):
what reaches a VTR file when a simulation dies, with and without a crash
guard, and the zstd facts the design relies on.

## Crash matrix

```sh
python3 bench/crashlab/run.py          # ~3 minutes; writes bench/results/crashlab.json
python3 bench/crashlab/run.py --quick  # one repetition for the timings
```

`run.py` builds `libvtr.a` twice: as it is, on the system allocator, and with
VTR on its own heap (`cargo build -p vtr-capi --features private-heap`, mimalloc
and direct mappings as the Rust global allocator). It links `crashlab.cpp` and
`check.cpp` against each, runs every scenario, and exits nonzero when an
outcome differs from the one the design page states.

`crashlab.cpp` is the guard reduced to its core. It has an async-signal-safe
handler on an alternate stack, a rescue thread that closes the writer, a hard
deadline, and a re-raise of the original signal. Stop requests and an `exit()`
hook are handled too. The simulated run writes 1,183 changes per time step
over 67,144 signals, the openC910 CoreMark rate, and a log record every 64
steps, and then dies. `check.cpp` reads the file back and reports whether it
was recovered by scanning, how many changes and log records it holds, and the
last FATAL message.

Pin to P-cores with `taskset -c 0-7`, which `run.py` does when available.
Core dumps are disabled for the children.

The same endings run on every push at a small size as the crash matrix,
`cargo test -p vtr-capi --test crash` (`core/vtr-capi/tests/crash.rs`), which
asserts what the file keeps for the library as it is. This directory stays
the full-size measurement of the prototype.

## zstd experiments

`zstd/` holds the programs behind the page's zstd section. They link against
the system libzstd (1.5.7 here) and take any 64 MiB input. The page used the
first 64 MiB of `bench/workloads/gen/ooo_1m.log`, of
`bench/workloads/gen/c910_coremark.rpl`, and of a gem5 debug trace.

```sh
cc -O2 zstd/sweep.c -lzstd -o sweep && ./sweep input      # one frame vs ZSTD_e_flush every N vs a frame every N
cc -O2 zstd/trunc.c -lzstd -o trunc && ./trunc input      # what a streaming decoder returns from truncated files
cc -O2 zstd/sig.c   -lzstd -o sig   && ./sig input        # compression inside a SIGSEGV handler, allocator calls counted
cc -O2 zstd/kill.c  -o kill && ./kill /path/on/fs/file    # SIGKILL during one large write(2): how much lands
```
