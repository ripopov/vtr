# VTR stdio query child

`vtr-server` is a per-document native child for the VS Code deployment in
[the client-server design](../../docs/client-server.md). It opens the trace on
its host and uses `vtr-query::local_session::LocalSession`, the same asynchronous
in-process adapter intended for native Volna. The current endpoint serves VTR
waveform windows, cold summaries, child/search/path pages and string byte parts.
The VS Code relay and viewer adoption are still under implementation; FST,
transaction operations, reader cache admission and warm indexes remain pending.

Build with the repository toolchain and Protocol Buffers compiler (`protoc` on
PATH, or `PROTOC` pointing to it):

```sh
cargo build -p vtr-server
cargo test -p vtr-server
cargo run -p vtr-server -- --stdio /absolute/path/to/trace.vtr
```

The endpoint tests also run the headless Volna demand scheduler through the
production RPC client and real child, including multiple rows, shared aliases
and display-list rendering. These development tests use the repository's pinned
viewer-capable Rust toolchain; building the server binary has no viewer dependency.

The final command expects binary protocol input, not interactive terminal text.
The host binds the trace path through an argument array. No trace path, URL,
credential or network listener is part of the wire protocol.

Each stdin/stdout frame contains a little-endian `u32` payload length followed
by a version-one Protobuf `Envelope` from
[`query.proto`](../../core/vtr-query/schema/query.proto). Stdout contains only
frames; diagnostics go to stderr. Send `Hello`, wait for `Welcome`, then send
`Open` and use its snapshot identity for queries, continuations and controls.
Request IDs must increase strictly within one child lifetime; retry a logical
continuation with a new request ID. This rejects ID reuse without retaining an
unbounded set of past IDs. A failed open can be retried against the same bound
path. A different trace needs a new child.

One query returns one page. Its `next` cursor requests another page; retrying
the current cursor returns the same logical delivery. Advancing acknowledges
the previous delivery. `Release` frees completed or abandoned operations;
completed operations continue to occupy one of four native operation slots until
release. `Cancel` targets an outstanding request ID and is acknowledged even
if that request has already finished. Cancellation cannot retract a frame already
being written. `Close` cancels work, prioritizes its acknowledgement and exits;
EOF disposes the session. An incompatible version, malformed frame or reused
request ID terminates the child. Invalid queries and resource exhaustion produce
correlated errors while preserving the session where possible.

The input pump, query worker and output pump run independently. Four data
requests may remain queued, executing or awaiting stdout delivery. Data credit
returns only after the output pump writes or discards the corresponding result.
Eight control messages have separate queue capacity, and the output pump checks
controls before starting another data frame. Query pages use at most 128 records
and 256 KiB of native reply storage even when the request permits more; an
individual result that cannot fit fails with `ResourceLimit`. The encoder also
checks the one-MiB wire and four-MiB decoded limits before transmission.

Queue/request/response allocations share a 64-MiB application budget; controls
have a separate 256-KiB allowance. Fixed channel and thread overhead, reader
caches, decoder scratch and OS pipe buffers are not a process-RSS guarantee.
The extension must bound its queues and stop reading stdout when webview delivery
credit is exhausted. It must also terminate a child after a bounded disposal
grace period when a blocked OS write or reader unit cannot finish cooperatively.

Process tests exercise framing, handshake, VTR pages, retries, operation limits,
cancellation, stale snapshots, malformed input, version rejection, EOF and close.
The production RPC driver is also exercised against the real child for every
currently implemented query family, including split UTF-8 text parts.
These tests do not replace the required live VS Code relay, sustained memory,
remote-workspace and native viewer performance checks.
