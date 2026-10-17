# VTR command-line tools

`vtr-cli` supplies the `vtr` executable and uses [`vtr-convert`](../vtr-convert/README.md) to import Kanata, VCD,
and FST. Use the pinned nightly toolchain in `rust-toolchain.toml` to build or
install from the workspace:

```sh
cargo build -p vtr-cli --locked
cargo install --path vtr-cli --locked
vtr --help
vtr convert --help
```

## Commands

`vtr convert` prints a progress bar with the current phase, input file size, and
current output VTR size on stderr. Interactive terminals update one line;
redirected stderr receives periodic lines. `--progress=none` suppresses this
display. `--progress=json` reserves stdout for versioned newline-delimited
[progress records](../vtr-progress/README.md), with diagnostics on stderr.
The output size includes the private file while conversion runs; the destination
is published only after conversion succeeds. Finalization stays indeterminate
while the writer flushes its buffers.

| Command | Input | Purpose |
| --- | --- | --- |
| `info FILE` | VTR | Metadata, time base, hierarchy counts, and sections |
| `hierarchy FILE [--vars] [--depth N] [--sizes]` | VTR | Declarations or scope counts; aliases count once per scope's signal total |
| `value FILE SIGNAL TIME` | VTR | Signal value at a tick |
| `changes FILE SIGNAL [--from T] [--to T] [--max N]` | VTR | Changes in an inclusive window |
| `transactions FILE [--scope PATH] [--from T] [--to T] [--max N]` | VTR | Transactions, attributes, events, and stages |
| `transactions FILE --id ID[,ID…]` | VTR | Selected transactions and their incoming/outgoing relations |
| `logs FILE [--scope PATH] [--severity LEVEL] [--from T] [--to T] [--max N]` | VTR | Formatted messages; `--sites` lists declarations instead |
| `clocks FILE` | VTR | Declared clocks and edge stretches |
| `convert INPUT OUTPUT [--format kanata\|vcd\|fst]` | Kanata, VCD, FST | Create a VTR recording |
| `to-vcd INPUT OUTPUT` | VTR | Export waveforms |
| `recover INPUT OUTPUT` | VTR | Copy verified sections into a complete recording |
| `index FILE [--check] [--threads N] [--memory MiB]` | VTR | Build or validate an activity sidecar |
| `active FILE [--from T] [--to T] [--build]` | VTR | List changing signals once each, using their first declaration |

Times are unsigned integer ticks in the file's time base, with both window
ends included. An omitted bound means zero or the largest tick. Signal and
scope paths use dots; quote paths containing spaces. Query output defaults to
100 records where `--max` is available. Text output is intended for terminal
inspection, not as a stable serialization protocol.

Conversion, export, and recovery require a new destination path. They publish
output only after successful completion. Exit codes are 0 for success, 1 for
`index --check` with no valid sidecar, 2 for invalid arguments or processing
errors, and 3 for successful recovery that dropped bytes.

Activity commands use the VTR reader and activity index directly. Sidecars are validated
against the source identity and written beside the trace, with the library's
user-cache fallback. `active` resolves undecided signals from the recording;
it requires an index or `--build`. `--memory` limits builder scratch, not total
process memory or the retained index. Keep recordings unchanged while reading.

## Conversion semantics and limits

- **Kanata 0004:** plain text or gzip, detected by its header. `C=`/`C` set or
  advance a nonnegative cycle; `I` creates an instruction; `L` labels the
  instruction, detail, or most recent stage; `S`/`E` delimit stages; `R` retires
  or flushes; `W` connects producer to consumer. Each thread is a pipeline
  generator using the period-one `cpu.cycle` clock. Ticks are abstract cycles
  (`time.unit=cycle`), not physical seconds. Repeated labels are newline joined.
  Instructions remain in memory until EOF for late labels and dependencies;
  unfinished instructions have Open status. Unknown commands, unsupported
  versions, dangling references, duplicate IDs, and backwards cycles are errors.
- **VCD:** standard declarations and the string extension, with aliases, bus
  ranges, four-state bits, repeated events, reals, and strings. Decimal
  timescale factors, producer version/date, and dump-control intervals are
  retained; a missing timescale defaults to nanoseconds. Values stream in
  timestamp order. Undeclared identifiers, incompatible aliases, overwide
  values, truncated commands, and backwards times are errors. Vendor-specific
  declarations, GHW, and EVCD are not supported.
- **FST:** the conversion library reads hierarchy, aliases, scope and variable
  types, components, input/output/inout directions, events, nine-state logic,
  reals, and raw string/port bytes. Other directions become implicit and root
  variables appear under `(top)`. Attributes, enum tables, and producer metadata
  are omitted. Nonzero time offsets and dump-off regions are rejected. All
  distinct histories must fit in memory. The CLI disables VTR deduplication to
  retain recorded samples.
- **VCD export:** supports two/four-state bits, events, and reals. Nine-state
  declarations, byte strings, unsupported timescales, time-zero offsets, and
  blackout intervals are rejected. Hierarchy names must be representable as VCD
  tokens, optionally followed by a bus range. Nonstandard scope kinds become
  modules and non-event bit declarations become wires. Transactions, logs,
  and declared clocks are omitted.

## Examples

Run from the workspace root; these use committed, hand-authored fixtures:

```sh
work=$(mktemp -d)
cargo run -p vtr-cli --locked -- convert vtr-convert/tests/fixtures/waves.vcd "$work/waves.vtr"
cargo run -p vtr-cli --locked -- value "$work/waves.vtr" 'top.bus [3:0]' 5
cargo run -p vtr-cli --locked -- active "$work/waves.vtr" --from 5 --to 7 --build
cargo run -p vtr-cli --locked -- to-vcd "$work/waves.vtr" "$work/waves.vcd"
cargo run -p vtr-cli --locked -- convert vtr-convert/tests/fixtures/pipeline.kanata "$work/pipeline.vtr"
cargo run -p vtr-cli --locked -- transactions "$work/pipeline.vtr" --scope cpu
cargo run -p vtr-cli --locked -- clocks "$work/pipeline.vtr"
```

Rust callers use `vtr_convert::kanata::convert_kanata`,
`vtr_convert::vcd::convert_vcd`, or `vtr_convert::fst::convert_fst` with a fresh
`vtr::Writer`, then explicitly close it.
Converters leave partial writer state after errors; discard that output.
`vcdout::write_vcd` writes to any `std::io::Write` sink. See the crate rustdoc
for a runnable example.

Convert FST to VTR before running `index` or `active`. The viewer and server
consume VTR data. Volna can invoke this CLI automatically when opening FST or
Kanata recordings; the server accepts the resulting VTR file.

Conversion fixtures and provenance are in
[`vtr-convert/tests/fixtures`](../vtr-convert/tests/fixtures/README.md).

## Verification and license

```sh
cargo test -p vtr-cli --locked
cargo clippy -p vtr-cli --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p vtr-cli --no-deps --locked
```

CLI tests cover queries, conversion commands, export, recovery, activity, and
failed-output cleanup. Conversion fidelity and malformed-input tests live in
`vtr-convert`. They require no simulator, network, GUI, or external checkout.

Licensed under MIT; see [LICENSE](../LICENSE).
