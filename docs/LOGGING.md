# Structured logging

A log source is a generator of kind `LOG`. Its name is the format string and its
payload datatype describes each message's values. Source metadata is declared
once; records store global ID, timestamp, optional ordinary owner, and the typed
payload. They use RECORD_BLOCK pages alongside located events and stages, as
specified in [TRANSACTIONS.md](TRANSACTIONS.md).

## Writing and reading

The runnable [logging example](../vtr/examples/logging.rs) defines a Rust `Payload`
structure with named text and numeric fields, captures messages, renders them,
and decodes retained payloads after dropping the reader. Its public operations
are `Writer::logs::<T>`, `Recorder::log`/`log_at`, `Reader::records`, and
`Reader::format_log`. C++ uses `Schema<T>`, `Writer::logs<T>`, and the corresponding
recording/reader operations; see the [C++ logging example](../vtr-cpp/examples/logging.cpp).

Capture copies borrowed values before returning. It does not format a message on
the producer thread. A scalar text payload and source name `{}` capture an
already formatted message; a structure preserves independently queryable fields.
Unit payloads have no format arguments. A log parent can identify the transaction
being processed, but logs do not appear in that owner's located-child locators.

`Reader::log_sources` exposes source metadata. `RecordQuery` selects generator,
kind, parent, ID, and inclusive time bounds. To filter by severity or scope,
select matching sources from metadata/hierarchy and query their generator IDs.
Block and segment headers prune unrelated data before page decompression.
Payload fields remain available through `PayloadView::field`, `scalar`, or typed
`decode`; no parsing of formatted text is required. Retained records keep their
schema and payload storage independently of reader caches and reader lifetime.

## Source metadata

| Key | Value type | Meaning |
| --- | --- | --- |
| `log.severity` | u64 | Trace 0, debug 1, info 2, warn 3, error 4, fatal 5; default info |
| `log.file` | str | Optional runtime source file |
| `log.line` | u64 | Optional runtime source line |
| `log.func` | str | Optional runtime function |

Other severity codes are preserved. Source provenance describes where the
runtime message originated; design-source mappings belong to VDB.

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

The Rust writer/guard can additionally emit one reserved source under the root
scope `vtr.run`. Its format is `run {}`, severity fatal, and attribute
`vtr.ending=true`. The `vtr.RunEnding` payload has these ordered fields:

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
