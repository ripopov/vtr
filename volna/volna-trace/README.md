# Volna trace loading

`volna-trace` is the standalone recording library used by `volna-core` and
`volna-server`. It owns VTR/FST readers, file and byte-image opening, immutable
hierarchies, complete signal histories and transaction tracks, activity
sidecars, memory admission, the raw protocol and its cooperative remote client.
It has no viewer, GUI toolkit or server-process dependency.

`Session` provides the common local/remote metadata and complete-object
contract. Local blocking queries run on the caller's executor and can share
reader storage directly. A remote session exposes the same immutable objects;
`remote::client::RemoteClient` queues raw loads, consumes framed responses and
returns acknowledgements, bounded decoder yields and complete results. Hosts
provide the transport and scheduler. Caller tags and generations correlate
results without entering the wire protocol or defining viewer state.

`volna-server` owns request handling, process identity, stdin/stdout hosting and
recording-change checks. Local loading opens a file in process and does not
start it. The existing protocol version 6 and VTR/FST support are unchanged.

Viewer demand, workspace state, timeline placement, activity classification,
value translation, transaction stacking and stage palettes belong to
`volna-core`. VDB profiles, source semantics, annotations and presentation
never enter this library's transport.

From the repository root:

```sh
cargo build --locked -p volna-trace
cargo test --locked -p volna-trace --all-targets
python3 tools/check-volna-loading.py
```

The last command checks the complete dependency graph, builds and tests both
standalone packages and runs strict Clippy. Reader tests use committed fixtures
in this package and `ext/surfer`; initialize workspace submodules as described
in the root README. CI runs the standalone checks on Linux, macOS and Windows.

Admission limits bound retained objects and construction allowances, not total
process RSS or reader caches. Complete metadata and selected histories/tracks
must fit in memory. Remote live-file updates, window queries and VDB attachment
remain outside this loading contract. The native local executor uses background
threads; local byte-image decoding on the existing single-threaded WASM host
can block. Remote decoding and viewer preparation yield cooperatively.
