# Structured logging

A **log site** declares a format string, severity, optional source provenance,
and argument types once. Each **record** stores a timestamp and typed values.
Sites are generators of kind `LOG`, optionally grouped in ordinary scopes. Records are zero-duration transactions
with unique transaction IDs, optional parents, and relation support.

Text arguments are deduplicated per block; the writer does not format messages
on the caller's thread. The reader reconstructs text or exposes typed values.
The on-disk encoding is [section 8 of the specification](SPEC.md#8-log_block-kind-8).

## Writing

```rust
use vtr::{LogArgType, LogSiteSpec, Severity, Timescale, Writer};

let mut writer = Writer::builder()
    .timescale(Timescale::Nanoseconds)
    .create("messages.vtr")?;
let scope = writer.add_scope(None, "simulation_log", vtr::ScopeType::Generic, "")?;
let site = writer.add_log_site(
    &LogSiteSpec::new(Some(scope), Severity::Warn, "{} stalled {} cycles",
                     &[LogArgType::Text, LogArgType::U64])
        .names(&["unit", "cycles"])
        .location(file!(), line!()),
)?;
writer.log(site, 100, &["lsu".into(), 7u64.into()])?;
writer.close()?;
```

`log` checks the argument count and types. `log_with_parent` attaches a record
to the transaction being processed. `LogArg` supports booleans, signed and
unsigned integers, floats, time, pointers, interned strings, inline UTF-8 text,
and raw bytes. For an already formatted message, use format `{}` with one
`Text` argument.

Log-site file/line/function fields identify a runtime message's origin. They do
not authorize embedding design-source mappings or presentation in VTR.

## Reading

```rust
use vtr::{LogQuery, Reader, Severity};

let reader = Reader::open("messages.vtr")?;
reader.visit_log(
    &LogQuery { min_severity: Severity::Warn, ..Default::default() },
    |record| {
        println!("{}: {}", record.time, record.format(reader.strings()));
        assert_eq!(record.arg(1), Some(vtr::LogArg::U64(7)));
        true // continue visiting
    },
)?;
```

`LogQuery` filters by scope subtree, generator, inclusive time window, and minimum
severity. Block headers prune irrelevant blocks before decompression.
`LogRecord::arg` exposes values; `format` and `format_into` render text.
`transaction`, `transactions`, and `visit_transactions` also expose log records
as transactions, with arguments keyed by the site's `log.names`.

## Format strings

The formatter accepts `{}`, explicit indices such as `{2}`, and specifications
such as `{:#010x}`, `{:>8.3}`, and `{:.2f}`. `{{` and `}}` escape braces.
Supported alignment is `<`, `^`, or `>`; integer radix types are `x`, `X`, `o`,
and `b`; float types include `e`, `E`, and `f`. Bytes render as hexadecimal.
A placeholder without an argument renders as `{?}`. See the `logfmt` rustdoc
for the supported subset; this is not the full Rust formatting language.

Run the complete log/transaction round trip:

```sh
cargo run -p vtr --example logging -- /tmp/logging.vtr
```
