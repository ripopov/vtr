# Volna server

The headless server opens one immutable VTR or FST recording and supplies its
complete metadata, selected signal histories, selected transaction tracks and
activity sidecars.
It depends on `volna-trace`, independently of the viewer core and frontends.
Opening and loading use that library's `OpenSpec` and
`Session` implementations as native Volna. The service adds framing, wire
conversion, backpressure and immutable-file checks. It borrows complete track
records during serialization; it keeps no second owned record cache.

Run with a protocol client:

```sh
cargo build -p volna-server --profile viewer
target/viewer/volna-server path/to/trace.vtr
```

Stdin and stdout carry `volna_trace::remote::transport` packets. Diagnostics go
to stderr. The first command is `Open`; the response assigns a fresh nonzero
session ID. Subsequent requests carry that ID and increasing request IDs.
Every response packet requires an acknowledgement before the next is sent.
Protocol 6 includes the activity source identity, cache availability and reader
host in its catalog. `Activity { build: false }` fetches a valid raw sidecar;
`build: true` authorizes the same VTR/FST builder to create a missing one. A
persistent sibling file lock and cache recheck coordinate server processes.
The sidecar is published atomically beside the trace or in the user's
identity-keyed cache for a read-only directory. A disconnected client may leave
a finished cache for others. Client cancellation stops installation, while the
already authorized server scan can finish. Classification and exact activity
counting stay on the client, using complete signal reads for undecided handles.
An object is complete only after its explicit end marker. `Close` or input EOF
releases the reader and exits. Detected changes to the file invalidate the
connection; live recordings are unsupported.

The packet format uses versioned framing, fixed little-endian bincode fields
and independent checksummed LZ4 frames. The client supplies the per-object
transfer limit at Open. This is a raw complete-object service; it has no
viewport, presentation, search or VDB endpoints. Client memory admission and
asynchronous object installation belong to `volna-trace`. The executable owns
request handling in `src/server.rs`, process hosting and snapshot checks; there
is no reusable server-library crate.

```sh
cargo test --locked -p volna-server
python3 tools/check-volna-loading.py
```

Process tests compare VTR/FST signal results with local sessions, including
aliases, and verify full transaction records, cross-generator relations,
empty generators, explicit errors, size limits, file-change invalidation, raw sidecar roundtrips and reuse between
two processes.
The VS Code deployment is described in
[the Volna architecture](../../volna/volna/ARCHITECTURE.md#complete-object-remote-loading).
