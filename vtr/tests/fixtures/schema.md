The JSON fixtures are emitted by `vtr-cpp/tests/engine/schema_test.cpp` using
`schema_payloads --fixture` and `schema_payloads --shared-fixture`. They retain
canonical rows, file-local datatype declarations, and all five argument columns.
The shared fixture additionally references the file string table. Rust checks
both decoding and byte-exact encoding, including signed enums, nested arrays and
sequences, NaN payload bits, packed logic, text reuse, and fixed-point values.
