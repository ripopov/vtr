# Recording transport

`ExecutionMode::inline_` invokes assembly directly on the simulator thread.
There is one implicit producer, no transport, no encoding handoff, and no worker
thread. Compression and file output complete on that caller. The output file
has exclusive ownership, so stdio's internal stream locking is disabled.
Both execution modes use the same schemas, lifetimes, wire encoding and reader.

The following protocol applies to `ExecutionMode::threaded`.

Each logical simulator producer owns a bounded SPSC byte ring. Recording calls
encode owned operations directly into its storage and publish every complete
operation with a release store. Signal scalars and time markers occupy 16 bytes.
Numeric and string-ID attributes occupy 24 bytes, including the owner ID, key,
value tag and scalar bits. Other operations carry a 40-byte header followed by
their payload bytes. Fixed-size operations write directly into reserved storage;
the collector applies scalar attributes without constructing a general operation.
The collector advances the reader cursor to release storage. There are no batch
objects or return queues on this path, and no shared-clock read per operation.

Advancing to a higher `T` appends a FIFO marker certifying that the producer
will never emit recording time below T again. Repeating the current time is a
no-op. The minimum producer watermark determines completed recording time.
Independent producers may advance at different rates; their observations are
merged by recording time while retaining each producer's FIFO order.

A single producer already establishes a total recording order. Its operations
can be applied directly from the ring, including operations at its current
uncompleted timestamp. A time marker advances assembly only after preceding
operations have been applied. Registering another producer switches to the
merge path and requires an initial time at or beyond the current frontier.

Record IDs identify creations independently of time ordering. Transaction
blocks sort completed rows by ID before page encoding; different blocks may
have overlapping ranges, so selective lookup examines every candidate block.
Creation reserves an ID with one atomic increment. The top 2^32 values are
reserved as an exhaustion guard and never published. An exhausted allocator
resets within that range before returning failure. Attachment precedes ID
reservation, limiting concurrent reservations to the maximum number of logical
producers; this bound prevents counter wrap and reuse even during concurrent
exhaustion. The counter occupies a separate cache line from backend control
polling.

Progress-only markers also advance the final recording time. Open records end
at the latest finite producer time even if the final ticks contain no changes.

With multiple producers, ingestion continues even when a watermark prevents
semantic serialization. The backend copies queued operations into reusable
byte storage and spills beyond the shared pending-byte budget to temporary
files. It preserves operations and time markers through replay. Draining does
not wait for the time frontier: a fast producer must be able to reach a
simulator barrier even when another producer cannot advance until that barrier.
Storage failures fail recording and surface to callers.

The default logical ring capacity is 256 KiB per producer. Its allocation is
twice the capacity so a record crossing the ring end remains contiguous; the
next record starts at its wrapped offset. Queue cursors and published positions
occupy separate cache lines. The producer caches the consumer position; the
consumer releases positions in byte groups and whenever the queue is drained.
Full queues apply backpressure without dropping observations. Wait statistics
count blocked publications and their duration, without reading a clock on the
uncontended path.

A payload larger than the ring uses an owned overflow allocation referenced by
one queue entry. At most one such allocation is outstanding per producer. The
backend consumes or stages its bytes before returning that credit. Ordinary
ring-sized operations allocate no transport storage. Public typed payloads up
to 256 bytes use inline encoding storage. Larger typed values are measured and
encoded directly into the reserved queue bytes, avoiding a temporary heap buffer
and its copy. Encoding finishes before publication and before the recording call
returns, so mutable borrowed input can be reused immediately. A failed encoder
leaves its reservation unpublished and fails recording. Signal spans up to 64
entries use a stack conversion buffer; larger spans and uncommon dynamic
attributes can allocate before publication. Backend schema, active-record, and
encoding storage have their own allocation and lifetime costs.

Logical producer registration and immutable declarations execute through a
backend control queue, making declarations visible before recording calls
reference them. Construction and initial declarations run on the constructing
thread; first recording or another thread's control request starts the backend.
Producer selection is cached per thread. Exclusive attachment transfers the
queue cursor and current recording time between threads, allowing simulator
event queues to migrate without losing their identity.

With zero encoding workers the backend writes sections directly. Optional
encoders claim tickets from a bounded ring and a separate output thread writes
ready sections in ticket order. Atomic reservations enforce the job and byte
budgets together; an oversized job occupies the queue exclusively. Encoding
workers return reusable payload storage through a multi-producer return stack.
The assembly backend alone consumes that storage. Encoding-job notifications
advance an atomic progress generation. Idle encoders, ordered output, and budget
waiters park with C++20 atomic waits; publication, completion, failure, and stop
wake them. They snapshot that generation before checking predicates to avoid
missing a transition. These notifications occur per encoding job, not per
recording operation.

Each backend pass bounds ingestion per producer and gives serialization a
matching total byte budget. A fixed global serialization budget would fall
behind even when every producer advances at the same rate, causing avoidable
staging and disk spill as the producer count increases. Full-queue waits yield
once to let a backend sharing the CPU run, then alternate processor pauses,
occasional yields, and short sleeps to limit CPU spent on prolonged stalls.

## Reference designs

The queue and stream implementation is original code. The following reference
designs inform its choices; no upstream queue headers are vendored.

| Concern | Binlog | Quill | VTR |
|---|---|---|---|
| Transport | Independent session channels with byte queues | Thread-local bounded or growing byte queues | Per-logical-producer byte rings |
| Publication | Commit complete entries with release/acquire cursors | Separate reservation, finish, and commit; cached remote cursors | Publish each complete operation; reuse consumed bytes |
| Growth | Replace a full channel and retain the old channel until consumed | Link new bounded queue nodes or apply the configured full-queue policy | Bound input queues and stage pending future data with a memory budget and spill |
| Ordering | FIFO within each channel; consumption can interleave channels | Merge timestamps with a grace period | Certified simulation-time watermarks and FIFO within each producer |
| Metadata | Consume source descriptions before events that reference them | Stable call-site metadata and backend reconstruction | Synchronous declaration publication before typed observations |
| Lifetime | Retain closed channels until drained | Publish thread invalidation before final cleanup | Explicit producer finish; retain queues until writer shutdown |

[Binlog revision 707f337][binlog] separates byte ownership from queue reader and
writer cursors. `beginWrite` reserves contiguous space and `endWrite` publishes
it; `beginRead` can expose two regions around a wrap. Its channel replacement
keeps old storage alive while the consumer drains it. Session registration and
metadata management use a mutex, so copying its channel transport would not by
itself satisfy a mutex-free recording interface.

[Quill revision 78c0696][quill] keeps local cursors and cached remote positions
on the respective queue sides, reducing shared-cache traffic. Its backend
stages decoded events and limits how much one frontend queue can monopolize a
drain pass. It also batches reader-cursor publication, including the final
partial batch when a queue empties. Its timestamp grace period addresses delayed
publication of wall-clock events; it cannot certify that a simulator has
finished a particular simulation timestamp. VTR requires explicit progress
instead of a wall-clock timeout.

The shared recording contracts and tracing ownership rules are explained in
[the rationale](../vtr/docs/RATIONALE.md#capture-and-output-have-separate-clocks-and-budgets).

[binlog]: https://github.com/erenon/binlog/tree/707f337f98427cdb1914f030b053101b77f2fd3a/include/binlog
[quill]: https://github.com/odygrd/quill/tree/78c0696c5be589488d882c280796dfa06bdcd492/include/quill
