# Structured logging

A logger is a named generator of kind `LOG`, independent of payload layouts.
Logging helpers describe statements with generic datatype attributes. Each
message records its logger, datatype, global record ID, timestamp, optional
ordinary owner and canonical payload. One logger can carry many datatypes, and
one metadata-bearing datatype can be used by several loggers. Identical messages
remain distinct occurrences.

## Writing and reading

Rust declares a stream with `Writer::logger(SourceOptions)`. The logging frontend
`vtr::logging::declare::<T>(&mut writer, CallsiteOptions)` declares a generic
one-field datatype whose `value` field references the reusable argument datatype
for `T`. The declaration carries the metadata below. Capture uses
`Recorder::log(logger, payload_type, &arguments, owner)` or `log_at` with an
explicit timestamp. `Writer::bind`, `Writer::declare_datatype` and `log_raw`
expose explicit datatype references and canonical payload bytes.

The core typed handle is `PayloadType<T>`; it contains a datatype ID and recording
identity. `Writer::datatype::<T>(DatatypeOptions)` constructs metadata-bearing
wrappers for any application, without interpreting their attributes. Datatype
references, source constraints and complete payload consumption are validated
by VTR. Different metadata declarations retain separate immutable IDs while
sharing their argument schemas.

C++ provides the corresponding `logger`, `datatype<T>`, `declare_datatype`, `log`,
`log_at` and `log_raw` operations. The optional `vtr_logging.h` frontend provides
`vtr::logging::declare<T>`, `declare_raw`, `metadata`, `callsites` and `format_log`.
The [C++ macro helper](../vtr-cpp/examples/logging.h) checks literal formats at
compile time and skips argument evaluation when the level is disabled. Immutable
static descriptors contain no recording-local IDs. Writer-owned registration
shares their datatype declarations across loggers; producer-local frontend
bindings retain typed handles and must be serialized by their logical producer.
A descriptor cannot change its layout or metadata within a writer.

Rust's `vtr::logging::callsites` and C++'s `vtr::logging::callsites` enumerate
metadata-bearing datatypes recognized by the logging frontend. They do not
enumerate hierarchy nodes. Rust's `vtr::logging::format_log[_bytes]` and C++'s
`vtr::logging::format_log` render messages. Formatting and logging-metadata
validation belong to these frontends. Malformed logging conventions remain
valid generic VTR metadata when their tagged encoding and references are valid.

`Record::datatype` identifies the selected schema; `RecordQuery::datatype`
filters located records by their effective datatype before decompression.
Severity comes from datatype metadata and scope from the logger hierarchy.
The argument view is `record.payload().field("value")` in Rust and
`record.payload()["value"]` in C++. Views retain their schema and captured bytes
after cache eviction and reader destruction. Borrowed values are captured before
recording returns. The [Rust example](../vtr/examples/logging.rs) and
[C++ example](../vtr-cpp/examples/logging.cpp) demonstrate deferred formatting.

## Datatype metadata conventions

The logging frontend recognizes `kind = "callsite"`. VTR itself gives this value
no special meaning. The wrapper's `value` field references the argument datatype.
Several wrappers can reference the same argument layout with different metadata.
A scalar argument supplies one formatting argument; a structure supplies its
fields in declaration order; unit supplies none.

| Key | Value type | Meaning |
| --- | --- | --- |
| `kind` | text or str | `callsite` |
| `log.format` | text or str | Required format string |
| `log.severity` | enum | Required: trace 0, debug 1, info 2, warn 3, error 4, fatal 5 |
| `log.file` | text or str | Optional runtime source file |
| `log.line` | u64 | Optional runtime source line |
| `log.func` | text or str | Optional runtime function |

Declaration helpers default severity to info. Severity attributes carry the numeric
code and its lowercase name. Other nonnegative codes through `i64::MAX` are
preserved with the name `other`.
Source provenance describes the runtime statement; design-source mappings belong
to VDB. Arbitrary application attributes can coexist with these conventions.
Logging lookups match key text and use the first matching attribute when distinct
string IDs contain the same text.
Volna recognizes the datatype marker and exposes logging metadata and the rendered
message as record attributes. The hierarchy contains the named logger stream.

LOG generators declare unit datatype. LOG segment headers store a datatype ID
after the generator ID; all records in a segment use that schema. Stable grouping
uses generator/datatype pairs without synthetic generators. Storage order need
not preserve the interleaving of different datatypes. Ordinary transactions,
EVENT and STAGE payloads remain fixed to their generators; embedded records keep
their attributes-only representation. These declarations require VTR 3.4; older
assets must be regenerated.

## Rendering

Structures supply arguments in field order. A non-unit scalar supplies one
argument; a unit supplies none. `{}` selects the next automatic argument and
`{2}` selects an explicit index. `{{` and `}}` escape braces. A missing argument,
invalid specifier, or unsupported aggregate argument renders `{?}`. An unmatched
opening brace retains the remaining source text.

Scalar presentations include decimal and radix integers; numeric booleans;
fixed, scientific, general, and hexadecimal floats; string precision; width;
sign and alternate forms; fill; and left/center/right alignment. Floating
rendering preserves f32 precision, signed zero, NaN signs, and infinity spelling.
String precision and padding use the first code point of each extended grapheme
cluster and the C++ [estimated field-width rules](https://eel.is/c++draft/format.string.std).
Integer `c` converts values representable by the platform's C `char` to one byte.
Rust's `format_log_bytes` preserves that byte exactly; `format_log` returns an
error when the result is not UTF-8. The C++ `L` option uses the global C++ locale.
Rust defaults to classic punctuation; `format_log_with_locale` and
`format_log_bytes_with_locale` take a `FormatLocale` for an individual call.
Its grouping, decimal point, thousands separator, and boolean names affect only
`L` presentations. Group sizes run from right to left, repeating the last size;
zero, a negative C `char`, or `CHAR_MAX` leaves the remaining leading digits
ungrouped. Separators are single bytes, and boolean names can contain arbitrary
bytes, matching `std::numpunct<char>`. Localized boolean labels use byte width;
text payloads use the grapheme rules above. Width and precision values greater
than 65,536 render `{?}`.
For example, `{:#010x}`, `{:>8}`, and `{:.2f}`. Bytes render as hex and packed
logic as state text. Fixed/ufixed render raw and scale as `raw` followed by `e`
and `scale`. Structured payloads remain inspectable even when a particular
presentation is unsupported. Formatting is presentation; the exact underlying
numeric bits remain in the payload.

## Run endings and crash rescue

Container completion and recovery provenance are defined in
[SPEC section 2](SPEC.md#2-container). They do not require a log message.

The Rust writer/guard can additionally emit one reserved logger `vtr.run.ending`
with attribute `vtr.ending=true`. Its message datatype wraps `vtr.RunEnding`, with format `run {}` and fatal
severity in datatype attributes. The wrapped argument has these ordered fields:

| Field | Type |
| --- | --- |
| message | text |
| reason | u8: stopped 1, exited 2, crashed 3, recovered 4, poisoned 5 |
| signal, code | i32 each |
| address, thread | u64 each |
| sealed | bool |
| status | i32 |
| dropped | u64 |

Unused fields are zero/false. Message text is derived from the typed ending;
readers validate that it agrees with the reason and fields. A normal close emits
no ending record. `Writer::close_with` records abnormal endings and marks the
container incomplete. `Reader::ending` returns the typed ending when available,
otherwise complete/recovery provenance from the container.

Independent threaded rescue stops new captures, wakes blocked producers, drains
accepted owned operations, and appends the ending. A busy inline writer or
panicked backend retains its verified file prefix instead. A missing ending log
therefore does not imply normal completion. The native C++ SDK currently has no
process guard; this typed ending convention is used by the Rust guard.

Actual Verilator logging, partial output, fatal exit callbacks, and binary-output
escaping are covered by [generated fixtures](../vtr/tests/fixtures/verilator/).
Run the Rust example with:

```sh
cargo run -p vtr --example logging -- /tmp/logging.vtr
```
