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
recording returns. The [Rust example](../vtr-rs/examples/logging.rs) and
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
| `log.format_style` | text or str | `brace` (default) or `printf`; interpreted by the C++ and Rust frontends |
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

### Brace formats

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

### Printf formats

Select `vtr::logging::FormatStyle::printf` in `CallsiteOptions` to retain a
printf format as datatype metadata and capture its arguments without formatting
the message. `format_log` dispatches on `log.format_style`;
`Reader::format_printf_arguments` also renders an argument payload directly.
The container and payload encodings are unchanged. Rust uses
`vtr::logging::FormatStyle::Printf` in `CallsiteOptions`; its
`format_log`, `format_log_bytes`, and `Reader::format_printf_arguments_bytes`
render the captured arguments. Volna uses this rendering for log messages and
retains the format style and original arguments alongside the message.

Native Rust readers use the same C library conventions as the C++ reader.
WebAssembly readers use UTF-8 for wide characters and strings, a dot decimal
separator without digit grouping, and hexadecimal pointer spelling. Their
floating presentations use captured f64 precision, including `L`.

```cpp
auto logger = writer.logger({.name = "GPUExec"});
auto site = vtr::logging::declare<std::uint64_t>(
    writer, {.format = "address=%#x", .severity = 1,
             .style = vtr::logging::FormatStyle::printf});
writer.log_at(logger, site, tick, std::uint64_t{0x8000});
// The reader renders "address=0x8000" from the captured value.
```

Supported presentations are `d i u o x X`, `f F e E g G a A`, `c s p`, and
`%%`, plus binary `b B`. Flags are `- + space # 0` and POSIX `'`. Width and
precision accept decimal constants or `*`; negative width implies left
alignment and negative dynamic precision is omitted. POSIX positions such as
`%3$*1$.*2$f` may repeat or reorder arguments; positional and sequential
indexing cannot be mixed. Integer precision, alternate prefixes, zero padding,
signed zero, infinities and NaNs follow the reader's C library. Locale-sensitive
formatting uses its current C locale, not the C++ global locale.

Arguments are typed values, not C varargs. Without a length modifier, integer
width and signed decimal representation come from the captured type: `%d`
does not truncate a captured u64 to an int. Unsigned presentations reinterpret
signed integers at their captured width. The `hh h l ll j z t` modifiers select
the corresponding integer width on the reader's ABI. Floating `l` and `L`
are accepted; `L` widens the captured f32/f64 to the reader's long double for
presentation, without introducing additional captured precision. Custom objects
must be adapted to a supported payload type before capture.

`%s` accepts captured text or an interned string ID and applies C byte precision
and NUL termination. `%c` accepts an integer and emits its low byte, including
NUL. `%lc` accepts a Unicode scalar value, and `%ls` accepts captured text
already encoded in the reader's multibyte locale; conversion and byte precision
follow `wcrtomb`/`printf`. `%p` accepts a captured address or integer and uses
the reader's pointer spelling. It never dereferences the address. A writer
capturing C string pointers must copy their contents as text, not store the
pointer for later dereferencing.

Malformed formats, mixed indexing, missing arguments, unsupported conversions
and incompatible argument types render `{?}`. `%n` also renders `{?}` and
never writes through a captured address. Unknown format styles are errors rather
than being rendered with the wrong dialect. The renderer bounds literal widths,
nonnegative precisions, individual converted fields and parsed format tokens to
65,536. Negative dynamic precision is ignored even at `INT_MIN`. Format size and
total output bytes additionally respect `ReaderOptions::max_payload_bytes`;
exceeding that limit raises `resource_limit`.

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
escaping are covered by [generated fixtures](../vtr-rs/tests/fixtures/verilator/).
Run the Rust example with:

```sh
cargo run -p vtr --example logging -- /tmp/logging.vtr
```
