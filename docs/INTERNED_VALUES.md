# Interned attribute values

Intern a complete typed value when many records share the same description.
An attribute then stores tag 25 and a varint value ID. Dynamic attributes stay
inline, and the producer chooses its lookup key and reuse policy. VTR does not
decode instructions or infer which values should be shared.

The value table preserves every attribute type, including integer widths,
floating-point bits, packed logic, bytes, lists, and maps. It shares content,
independently of canonical payload schemas. Lists and maps may reference earlier
interned values. Values and IDs are immutable and local to one recording.

## Rust

```rust
use vtr::{Attribute, Recorder, Value, Writer};

let mnemonic = writer.intern("mnemonic")?;
let pipe = writer.intern("pipe")?;
let instruction_key = writer.intern("instruction")?;
let mask_key = writer.intern("exec_mask")?;
let description = writer.intern_value(Value::Map(vec![
    Attribute { key: mnemonic, value: Value::Text("FADD".into()) },
    Attribute { key: pipe, value: Value::Text("FP32".into()) },
]))?;

writer.attribute(owner, instruction_key, Value::Ref(description))?;
writer.attribute(owner, mask_key, Value::U64(0xff))?;
```

Retain `description` for subsequent transactions. Passing the same typed content
to `intern_value` returns the same `ValueId`, but attaching an existing ID needs
no lookup. `Recorder::intern_value` is also available on a threaded producer.

`reader.value(id)` borrows a table entry. `reader.resolve_value(value)` follows
references until it reaches a non-reference value; references nested inside a
list or map remain available to the caller. Neither method copies a description
into each transaction.

## C++

```cpp
const auto mnemonic = writer.intern("mnemonic");
const auto pipe = writer.intern("pipe");
const auto instruction_key = writer.intern("instruction");
const auto mask_key = writer.intern("exec_mask");
const auto description = writer.intern_value(vtr::Value::Map{
    {mnemonic, vtr::Value(std::string("FADD"))},
    {pipe, vtr::Value(std::string("FP32"))},
});

writer.attribute(transaction, instruction_key, vtr::Value(description));
writer.attribute(transaction, mask_key, std::uint64_t{0xff});
```

`ValueId` is a distinct alternative in `Value::Storage`.
`reader.value(id)` and `reader.resolve_value(value)` return `const Value&` whose
lifetime is the reader's lifetime. Retained records preserve reference IDs after
the reader closes; resolving those IDs still requires their recording's reader.

## Identity and publication

Interning compares the encoded typed content: types, float bits, ordered keys
and elements, string IDs, and nested value IDs. It distinguishes signed zero,
NaN bit patterns, inline text, and interned strings. It does not merge a map
with reordered keys or a reference with its expanded content.

Declare referenced strings and values before interning their users. Values may
refer only to earlier entries; forward references and cycles are rejected.
Writers publish compressed value chunks before dependent metadata or runtime
sections, including across flushes. Recovery retains complete chunks and the
references in surviving sections. See the [format specification](SPEC.md#54-interned-values).

Interning has a table and lookup cost. Retaining IDs avoids repeated lookups;
unique or very small descriptions may be cheaper inline. Existing schema payload
columns already share repeated text, so a string field alone does not always
benefit from moving into an interned object.

The [measurement](../benchmarks/intern-values.md) compares complete compressed
files containing identical instruction descriptions and dynamic masks. Reproduce
it on Linux from the repository root:

```sh
python3 benchmarks/build.py
python3 benchmarks/intern_values.py
```

The [validation report](INTERNED_VALUE_VALIDATION.md) records interoperability,
regression measurements, and the known baseline GUI test failures.

The [Rust transaction tutorial](../vtr-rs/examples/transactions.rs) and
[C++ transaction tutorial](../vtr-cpp/examples/transactions.cpp) show references
alongside payloads, stages, and relations.
