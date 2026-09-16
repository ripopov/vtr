# Volna server

The headless server opens one immutable VTR or FST recording and supplies its
complete metadata, selected signal histories and selected transaction tracks.
It imports no GUI toolkit. Opening and loading use the same `OpenSpec` and
`Session` implementations as native Volna. The service adds framing, wire
conversion, backpressure and immutable-file checks. It borrows complete track
records during serialization; it keeps no second owned record cache.

Run with a protocol client:

```sh
cargo build -p volna-server --profile viewer
target/viewer/volna-server path/to/trace.vtr
```

Stdin and stdout carry `volna_core::remote::transport` packets. Diagnostics go
to stderr. The first command is `Open`; the response assigns a fresh nonzero
session ID. Subsequent requests carry that ID and increasing request IDs.
Every response packet requires an acknowledgement before the next is sent.
An object is complete only after its explicit end marker. `Close` or input EOF
releases the reader and exits. Detected changes to the file invalidate the
connection; live recordings are unsupported.

The packet format uses versioned framing, fixed little-endian bincode fields
and independent checksummed LZ4 frames. The client supplies the per-object
transfer limit at Open. This is a raw complete-object service; it has no
viewport, presentation, search or VDB endpoints. Client memory admission and
asynchronous object installation belong above the packet decoder.

```sh
cargo test --locked -p volna-server
```

Process tests compare VTR/FST signal results with local sessions, including
aliases, and verify full transaction records, cross-generator relations,
empty generators, explicit errors, size limits and file-change invalidation.
The VS Code deployment is described in
[the design](../../docs/client-server-simple.html).
