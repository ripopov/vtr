# Signal history loading: FST and VTR memory

Volna's FST loader stored every change as a separate `String`. A 100M-change
clock in `volna/volna/examples/large_fst.fst` counted 5.2 GiB and failed the
object limit, while Surfer (wellen) loads all four signals of that file in about
3 GB. FST logic and real signals now load into packed histories, which use less
memory than wellen and are as fast or faster to query. This document records
what was measured, what was built, and the optional steps that remain.

## Current layout

`data::compact::CompactHistory` holds `u64` change times and fixed-stride packed
values in VTR's logic codes (1, 2 or 4 bits per bit for 2, 4 or 9 states).

- Entries of at most 8 bits share bytes, so a two-state clock costs one bit per
  change for its values.
- Wider entries are byte-aligned, so a `ValueView` borrows them in place.
- A signal starts two-state and is repacked when a value needs more states.
- Buffers are trimmed after the load, and `resident_bytes` counts real capacities.

Text and event signals keep `VecHistory`. VTR sessions use `VtrHistory` over the
reader's `SignalData`, which also stores `u64` times and packed values.

## Measurements

`history_cost` runs on `large_fst.fst`, pinned to P-cores of an Intel Core
Ultra 7 265K, one configuration per process. Each signal has 100M changes
unless noted. fst-reader alone decodes `clk` in 0.9 s and `sine_100m` in 2.7 s.

| Load | Before | After |
|---|---|---|
| `clk` (1-bit) | 3.4 s, 54.7 B/change counted, 72 B RSS | 1.36 s, 8.1 B counted, 8.6 B RSS |
| `sine_100m` (32-bit) | 10.7 s, 85.7 B/change counted, 88 B RSS | 4.4 s, 12.0 B counted, 12.1 B RSS |
| All four (211M changes) | 16.0 s, +16.2 GiB RSS | 5.8 s, +2.1 GiB RSS |
| wellen, all four, for comparison | 0.75 s open + 8.8 s, +2.9 GiB RSS | |

Point queries (about 400 ns) and 1400-column frame sweeps (about 100 µs over
the whole signal) are unchanged. Random numeric decodes of `sine_100m` fell
from 608 ns to 51 ns.

The time layouts were compared on `sine_100m` (dense: a change at every step) and
on every 97th of its changes spread over the same 1e8 steps (sparse):

| Layout | Point query, dense / sparse | Frame sweep, dense / sparse | Scan, dense / sparse |
|---|---|---|---|
| `u64` times (current) | 395 / 38 ns | 87 / 16 µs | 0.94 / 0.56 ns |
| wellen: `u32` index + shared table | 1072 / 868 ns | 221 / 171 µs | 0.88 / 7.4 ns |
| base + `u32` offsets | 318 / 30 ns | 70 / 21 µs | 0.67 / 0.59 ns |

A shared time table only pays off when many dense signals share their steps.
It costs its full size and build time at open (763 MiB and 0.75 s here), caps
traces at 2^32 steps, and makes lookups 3–30× slower. It stays rejected (see
`docs/RATIONALE.md`, "Volna FST session integration"). The full results are
in `docs/RATIONALE.md`, "Volna FST signal histories".

## Remaining optional steps

These steps would roughly halve memory again. Loading and queries already perform
well, so they are not scheduled.

1. **Accurate VTR accounting.** `VtrHistory::resident_bytes` uses a width
   formula: it counts 9 B per change for `clk` against about 18 B of RSS.
   Report real capacities, trim the reader's builders, and release its
   decompressed-piece cache after a batch load.
2. **Compact times.** Store times as a `u64` base plus `u32` offsets, falling
   back to `u64` when a signal's span exceeds 2^32 units. This saves 4 B per
   change and is slightly faster to query. For `vtr::SignalData` it replaces
   `times() -> &[u64]` (28 call sites in the C API, `vtr-vdb`, Volna, Surfer's
   adapter and tests) and needs the `bench/run.py` A/B, since it changes the
   reader decode path. `CompactHistory` would adopt the same layout.
3. **Smaller VTR values.** Pack 1-bit two-state VTR values eight per byte, add a
   fast four-state path to `LogicView::to_u64` (87 ns per change today for
   32 bits), and make `vtr convert` pick two states for two-state FST content.

With all three steps, the four signals would take about 1.3 GB.

## Reproducing

Generate the trace as described in `volna/volna/examples/README.md`, then:

```sh
taskset -c 0-7 cargo run --release -p volna-core --example history_cost -- \
  volna/volna/examples/large_fst.fst clk sine_1m sine_10m sine_100m
```

With the default limits (`memory.objectMiB` 256, `memory.budgetMiB` 512), the
viewer still refuses `clk` (775 MiB) and `sine_100m` (1.1 GiB). Raise the
limits in settings to load them.
