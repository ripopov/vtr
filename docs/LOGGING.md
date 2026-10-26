# Structured logging

A logger is a named generator of kind `LOG`, independent of message argument
layouts. A callsite declares a format string, argument datatype, severity and
optional source provenance. Each message records its logger, callsite, global
record ID, timestamp, optional ordinary owner, and canonical argument payload.
One logger can carry many callsites, and one callsite can be used by several
loggers. Identical messages remain distinct occurrences.

## Writing and reading

Rust declares a stream with `Writer::logger(SourceOptions)` and a typed statement
with `Writer::callsite::<T>(CallsiteOptions)`. Capture uses
`Recorder::log(logger, callsite, &arguments, owner)` or `log_at` with an explicit
timestamp. `Writer::bind` and `declare_callsite` expose datatype references;
`Recorder::log_raw` accepts a logger, callsite ID, timestamp and canonical bytes.
The raw path validates the declared datatype and consumes the entire payload.

C++ provides the corresponding `logger`, `callsite<T>`, `declare_callsite`,
`log`, `log_at` and `log_raw` operations. The [C++ logging helper](../vtr-cpp/examples/logging.h)
checks literal formats at compile time and skips argument evaluation when the
level is disabled. Callsite descriptors are immutable. Writer-owned registration
reuses the same callsite across loggers; producer-local logger bindings retain
stable typed handles. Static descriptors contain no recording-local IDs.
Separate producer bindings can share a logger handle without sharing mutable
frontend caches. Registration finishes before steady capture, which uses direct
handles without metadata construction or type matching.

The [Rust example](../vtr/examples/logging.rs) and
[C++ example](../vtr-cpp/examples/logging.cpp) capture values and format them later.
Borrowed text and arrays are copied or encoded before return. A scalar text
payload and callsite format `{}` capture already formatted output; a structure
preserves independently queryable fields. Unit payloads have no format arguments.
Log owners do not add EVENT/STAGE child-block locators.

Rust `Reader::log_callsites` and C++ `Reader::callsites` expose formats, argument
datatypes, severity and provenance before decoding messages. `Record::callsite`
retains the declaration reference. `RecordQuery` selects logger generator,
callsite, kind, parent and inclusive time bounds; Rust also exposes an ID filter.
Select severity from the callsite catalogue and scope from the logger hierarchy.
Segment headers prune unrelated logger/callsite groups before decompression.
`PayloadView` supports typed fields and decoding independently of formatting,
and retains both schema and storage after reader destruction.

## Callsite declarations

Callsites use HIERARCHY kind 5 in the shared node ID namespace. Their name is the
format string, their payload reference is the argument datatype, and their parent
must be absent. They are omitted from design-hierarchy children and are not
generators. Distinct declarations remain distinct even when all metadata and
datatypes match.

| Key | Value type | Meaning |
| --- | --- | --- |
| `log.severity` | u64 | Required: trace 0, debug 1, info 2, warn 3, error 4, fatal 5 |
| `log.file` | str | Optional runtime source file |
| `log.line` | u64 | Optional runtime source line |
| `log.func` | str | Optional runtime function |

Declaration APIs default severity to info. Other codes are preserved. Duplicate
reserved metadata keys and values of the wrong type are invalid. Source
provenance describes the runtime statement; design-source mappings belong to VDB.

LOG generators declare unit datatype. Each LOG segment adds its callsite ID
after the generator ID; all records in that segment use the callsite's datatype.
The writer groups by logger/callsite without introducing additional generators.
Storage order need not be chronological or preserve the interleaving of different
callsites. Ordinary transaction, EVENT and STAGE payloads remain fixed to their
generators; embedded records continue to carry attributes without datatype schemas.
These declarations and the segment selector require VTR 3.3; older assets must
be regenerated.

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
with attribute `vtr.ending=true`. Its callsite format is `run {}` and severity is
fatal. The `vtr.RunEnding` payload has these ordered fields:

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
