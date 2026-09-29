# Hierarchy open A/B measurements

These files are the complete local/remote Stage 2 cohort, including all three
samples per workload. The fastest complete open selects the entire reported row.
`bench/run.py report` renders both cohorts into `docs/BENCHMARK_RESULTS.md`.
The baseline is `0de8b057273e99fed55419513539e1c58f6a46d3`. The small
`baseline-harness.patch` repairs that commit's measurement example for its
existing multi-trace API; it changes no measured library code.

Use the same machine, release profile, input files and session for both builds.
On Linux, pin to P-cores 0–7; the runner alternates baseline and changed binaries
for each sample and provisions fresh child processes,
measures zero selected histories, enforces a 300-second process timeout and
rejects unsuccessful loads. The server examples kill and reap their child.
No browser or UI process is involved in these native measurements.

Generate fixtures from the benchmark's C910 recording (`bench/run.py` provisions
it), with the checked-in `vtr-bench gen-gates` command. For N = 1, 4, 16:

```sh
cargo build --release -p vtr-bench -p vtr-cli
# Substitute 1, 4 and 16 for N.
target/release/vtr-bench gen-gates bench/results/latest/c910_coremark.vtr gatesN.vtr --copies N
# FST twins for N = 1 and 4; vcd2fst comes from GTKWave.
target/release/vtr to-vcd gatesN.vtr gatesN.vcd
vcd2fst gatesN.vcd gatesN.fst
```

Prepare the baseline checkout, initialize its `ext/elkrs` dependency, apply the
harness patch and build it with its own `CARGO_TARGET_DIR`. Build the changed
checkout with the same command:

```sh
cargo build --release -p volna-core --example load_cost -p volna-server --example remote_cost --bin volna-server
python3 bench/hierarchy-cost.py --bin-dir target/release --baseline-bin-dir BASELINE_TARGET/release --fixtures FIXTURES --output bench/results/hierarchy-stage2/after.json --baseline-output bench/results/hierarchy-stage2/before.json
python3 bench/run.py report
```

Other builds were using the measurement host during this cohort. Interleaving
and best-of-three sampling reduce the resulting timing variation, but absolute
wall times and longest-step times include scheduling delays. The files retain
all samples and machine information so this limitation is visible.

The files retain source paths as diagnostics; reproductions may use any fixture
directory. `trace_bytes` records unchanged input size. Hierarchy storage changes
no trace writer, encoding or on-disk bytes. Remote wire bytes include compressed
framing, the raw catalog, all declaration pages and scope sizes. Native decoder
steps do not measure WASM execution time; the separately checked-in browser
regression tests functionality on WASM without inferring its speed from these
results.
