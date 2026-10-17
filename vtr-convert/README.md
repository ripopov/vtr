# VTR conversion

`vtr-convert` imports FST, Kanata, and VCD into VTR without depending on Volna's
viewer or trace loader. The [`vtr` CLI](../vtr-cli/README.md) supplies argument
parsing, native files, and atomic destination publication.
[`vtr-convert-wasm`](../vtr-convert-wasm/README.md) hosts the same importers in a
browser worker with in-memory input and output.

Call `fst::convert_fst`, `kanata::convert_kanata`, or `vcd::convert_vcd` with an
input path, a fresh `vtr::Writer`, and a `vtr_progress::Reporter`, then close the writer. Discard output after
any conversion error. For FST, disable writer deduplication to retain recorded
samples, including repeated events. Foreign-format decoding dependencies belong
in this crate; the viewer and `volna-server` read VTR only.

The `convert_input` entry points accept readers instead of paths. FST needs a
buffered, seekable reader; Kanata and VCD need `Read` and the original input
length for progress. Kanata detects gzip content. The browser host unwraps
external gzip FST and VCD before invoking their readers.

See the [CLI guide](../vtr-cli/README.md#conversion-semantics-and-limits) for
conversion semantics and limits. FST retains complete signal histories, and
Kanata retains instructions until EOF; these conversions require those values
to fit in memory.

Progress has separate reading and writing phases. Kanata and VCD count source
bytes read, including compressed bytes for gzip Kanata. FST reading counts
simulation ticks through the recording; writing counts emitted samples. Kanata
writing counts retained instructions. Preparation and writer finalization have
unknown totals. Hosts measure the output file on disk independently of these
work counters.

```sh
cargo test --locked -p vtr-convert
cargo clippy --locked -p vtr-convert --all-targets -- -D warnings
```

Tests cover conversion fidelity, aliases, raw byte strings, repeated events,
time bases, pipeline transactions, compressed inputs, and malformed recordings.
Committed [fixtures](tests/fixtures/README.md) need no simulator or submodule.
