# Logging with VTR

Simulators print a lot of text: UVM reports, `$display`, SystemC
`SC_REPORT_INFO`, a model's own `LOG_INFO(...)` macros. VTR stores those
messages next to the waveforms and transactions of the same run, in the same
time base, at about a tenth of the size of the text and without formatting
on the simulator's thread. This guide covers the model, the Rust and C/C++
APIs, reading the messages back, and what to expect in performance. The
on-disk format is section 8 of `SPEC.md`; the design history and the
measurements against NanoLog, binlog, Quill and CLP are in `RATIONALE.md`
and `BENCHMARK_RESULTS.md`.

## 1. The model

A logging statement in the source (`LOG_INFO("axi write addr={:#x} len={}", a, n)`)
is a **log site**: a format string, a severity, a source location and a list
of argument types. Every execution of the statement is a **log record**: a
timestamp and the argument values.

In the file, a log site is a *generator* of a stream of kind `LOG`, named by
its format string and carrying the static facts as attributes
(`log.severity`, `log.args`, `log.names`, `log.file`, `log.line`,
`log.func`). A log record is a *zero-duration transaction* of that generator:
it has a transaction id, a time, optionally a parent transaction (the
transfer or instruction being processed when the message was produced) and
its arguments as attributes. Records are stored in `LOG_BLOCK` sections that
hold only the argument values, column-wise, with a per-block dictionary for
repeated strings, compressed with the file's codec.

Consequences:

* No formatting happens in the simulator. The hot path copies the argument
  values (a few bytes) into a buffer; encoding and compression run on
  background threads.
* Text is reconstructed by the reader (`vtr log`, `LogRecord::format`,
  `vtr_log_rec_format`), so a viewer can also filter, sort and aggregate the
  *values*: all bus errors above an address, the longest stalls, the messages
  of one transaction.
* Because records are transactions, they appear in `visit_transactions`
  next to the other transactions of the run, can be relation endpoints, and
  need no extra support in a VDB or viewer that already understands
  transactions.
* The message severity is a property of the site, so a reader skips whole
  blocks that contain no site at the requested level without decoding them.

Argument types (`LogArgType`, `VTR_VAL_*`): `bool`, `i64`, `u64`, `f64`,
`time`, `pointer`, `text` (UTF-8, deduplicated per block: names, states,
responses), `str` (an id the producer interned itself with `intern`, for
strings that must be stable across the file), `bytes` (raw payload, stored
verbatim). Integers are stored in a variable-length encoding, so small values
cost one byte and 64-bit addresses five or nine.

## 2. C++: `vtr_log.hpp`

```cpp
#include "vtr.h"
#include "vtr_log.hpp"

vtr_writer *w = vtr_writer_create("sim.vtr", nullptr);
uint32_t cpu = vtr_writer_begin_scope(w, "cpu0", 68 /* core */, nullptr);
vtr::LogStream log(w, cpu, "log");            // a LOG stream under cpu0
vtr_writer_end_scope(w);

VTR_LOG_INFO(log, sim_time, "fetch pc={:#010x} inst={:#010x}", pc, inst);
VTR_LOG(log, vtr::Severity::Warn, sim_time, "{} stalled {} cycles", unit_name, cycles);

log.set_parent(tx_id);                         // following messages belong to this transaction
VTR_LOG_ERROR(log, sim_time, "bus error at {:#x} ({})", addr, "SLVERR");
log.set_parent(0);
vtr_writer_close(w);
```

`VTR_LOG(stream, severity, time, "format", args...)` and the
`VTR_LOG_TRACE/DEBUG/INFO/WARN/ERROR/FATAL` shorthands register the call
site on first execution (format string, severity, `__FILE__`, `__LINE__`,
`__func__` and the argument types deduced from the C++ types) and then store
the arguments. The number of `{}` placeholders is checked against the number
of arguments at compile time. Type mapping: `bool`; signed integers and
enums to `i64`; unsigned to `u64`; `float`/`double` to `f64`;
`const char*`, `std::string`, `std::string_view` to `text`;
`vtr::interned{id}` to `str`; `vtr::bytes{p, n}` to `bytes`;
`vtr::pointer{v}` or any `T*` to `pointer`; `vtr::timestamp{v}` to `time`.
`LogStream::set_min_severity` drops messages below a level before they reach
the writer. Neither the writer nor a `LogStream` is thread-safe; log from the
thread that owns the writer.

The header is C++17 and uses only the C API underneath
(`vtr_writer_add_log_site`, `vtr_writer_log_raw`), so a C or SystemC project
can call those directly; `API_C.md` documents them.

## 3. Rust

```rust
use vtr::{LogArgType::*, LogSiteSpec, Severity, Writer};

let mut w = Writer::create("sim.vtr")?;
let cpu = w.begin_scope("cpu0", vtr::ScopeType::Core, "");
let log = w.add_log_stream(Some(cpu), "log");
w.end_scope()?;
let fetch = w.add_log_site(&LogSiteSpec::new(log, Severity::Debug, "fetch pc={:#010x} inst={:#010x}", &[U64, U64])
    .names(&["pc", "inst"]).location(file!(), line!()));
let stall = w.add_log_site(&LogSiteSpec::new(log, Severity::Warn, "{} stalled {} cycles", &[Text, U64]));

w.log(fetch, t, &[pc.into(), inst.into()])?;
w.log_with_parent(stall, t, Some(tx), &["lsu".into(), 7u64.into()])?;
w.close()?;
```

`add_log_site` returns a `LogSiteId` (a dense index; `log_site_node` gives
the generator). `log` checks the argument count and types against the site
and returns the record's transaction id. `LogArg` implements `From` for the
integer and float types, `bool`, `&str` (as `Text`), `&[u8]` (as `Bytes`) and
`StrId` (as `Str`). `log_raw` takes arguments already in row encoding for
front ends that encode themselves (the C++ header does).

## 4. Format strings

Placeholders are `{}` (next argument), `{2}` (explicit index) or
`{:spec}` / `{2:spec}`, with
`spec = [[fill]align][sign][#][0][width][.precision][type]`:
align `<` `^` `>`, sign `+`, `#` for the `0x`/`0o`/`0b` prefix, `0` for
zero padding, `type` one of `x X o b` (integers), `e E f` (floats),
`s`. `{{` and `}}` are literal braces. This is the subset shared by Rust's
`std::fmt` and C++ `std::format`, so the same literal compiles with `fmt`
and renders identically from VTR (`{:#010x}`, `{:.2f}`, `{:>8}`). Integers
in a non-decimal radix render as two's complement; a float without a
precision prints its shortest round-trip form; `bytes` render as hex; a
placeholder without an argument renders as `{?}`.

## 5. Reading

Command line:

```sh
vtr log sim.vtr                                  # every record: time, severity, stream, text
vtr log sim.vtr --severity warn --from 0 --to 1000000
vtr log sim.vtr --stream soc.cpu0.log --max 100
vtr log sim.vtr --sites                          # the call sites and their argument types
vtr tx sim.vtr --id 4711                         # a record (or transaction) by id, with relations
```

Rust:

```rust
let r = vtr::Reader::open("sim.vtr")?;
r.visit_log(&vtr::LogQuery { min_severity: Severity::Warn, ..Default::default() }, |rec| {
    println!("{} {}: {}", rec.time, rec.severity().name(), rec.format(r.strings()));
    if let Some(vtr::LogArg::U64(cycles)) = rec.arg(1) { /* the value itself */ }
    true
})?;
for site in r.log_sites() { /* node, stream, severity, fmt, file, line, args, names */ }
```

`LogQuery` filters by stream, site (generator), minimum severity and time
window; block headers prune before decoding. `LogRecord` gives `id`, `time`,
`parent`, `site`, typed `arg(i)`/`args()`, `format_into` and
`to_transaction`. `visit_transactions` and `transaction(id)` return log
records as transactions too (attributes keyed by `log.names`).

C: `vtr_reader_visit_log(r, stream, generator, min_severity, t0, t1, cb, user)`
delivers `vtr_log_rec` (id, time, parent, site index, severity, argument
count); `vtr_log_rec_arg` reads one argument as a `vtr_value`
(`VTR_VAL_TEXT` points into the reader), `vtr_log_rec_format` renders the
text like `snprintf`; `vtr_reader_log_site` / `vtr_reader_log_site_arg`
describe the sites. `vtr::for_each_log` and `vtr::format_log` in
`vtr_log.hpp` wrap these for C++.

## 6. Performance guide

* **Hot path.** One record costs the argument bytes plus a few varints
  appended to a buffer: about 30 ns for three arguments from C++ or Rust on
  an Apple M5 (30 million messages per second), no allocation, no
  formatting, no hashing. Interning (`str`) does hash; `text` does not.
* **Encoding** runs on `log_encoders` helper threads (default 2) fed by the
  writer's background thread: column split, per-block text dictionary and
  compression. Log blocks are independent and may be written out of order,
  so the helpers pipeline freely. With `background: false` everything runs
  on the caller's thread.
* **Block size** follows `tx_block_bytes` (default 4 MiB of rows, roughly
  150k messages). Larger blocks compress slightly better; smaller blocks
  make time-window and severity queries more selective.
* **What compresses.** Repeated text arguments cost one byte after the
  dictionary; small integers one byte; monotonic time deltas one or two
  bytes; random 64-bit data (payloads, hashes) is incompressible in any
  format. On the benchmark's SoC log the file is about 10 bytes per message
  against 66 for the text log, 17 for zstd-compressed text, 26 for NanoLog
  and 15 for a CLP IR stream.
* **Reading back** renders about 10 million lines per second; a severity
  or stream query that touches few sites is limited by block pruning, not
  by decoding.

## 7. Mapping existing loggers

| source | site | record |
|---|---|---|
| UVM `uvm_info(id, msg, verbosity)` | one site per report location; `id` and verbosity as attributes or as `text`/`u64` arguments; the component path is the stream's scope | time = `$time`, message as one `text` argument when already formatted, or the `$sformatf` pieces as typed arguments |
| SystemC `SC_REPORT_*` | one site per (message type, severity) | `sc_time_stamp()`, the message text |
| C/C++ `LOG_*(fmt, ...)` macros | one site per macro expansion (what `vtr_log.hpp` does automatically) | the arguments |
| an existing text log | one site per distinct format (a converter can recover it the way CLP does) | the extracted variables |

A message that is already a formatted string is stored as a single `text`
argument of a site whose format is `{}`; repeated messages still cost one
dictionary index each.
