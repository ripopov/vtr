# VTR crash guard

`vtr-guard` is an opt-in Linux process guard for VTR writers. A fatal signal
hands watched writers to a rescue thread, which closes writers whose owners are
at rest and seals writers interrupted inside a call. Stop requests can be polled
so a simulation can finish its time step and close normally. The guard also
handles `exit()` with watched writers. `SIGKILL` cannot be intercepted.

Call `install` once, then `watch` each writer from its owner thread. Call
`unwatch` before closing it. A watched thread receives an alternate signal
stack; call `thread_init` on other threads that may fault. `Options` controls
the rescue deadline, stop grace period, and enabled handlers. The guard is
process-wide and supports at most 64 watched writers. `VTR_GUARD=0` disables
installation; `VTR_GUARD_DEADLINE_MS` and `VTR_GUARD_STOP_GRACE_MS` override the
corresponding durations.

Rust writers use the public `vtr::Writer` and typed `Recorder` API. Threaded
rescue stops new captures, wakes blocked producers, drains accepted operations
(including spilled queues), and writes an incomplete directory with a typed
run-ending log. Explicit producer handles may still be live during rescue.

Idle inline writers can be closed after their owner is parked. A writer
interrupted inside an inline call retains its verified file prefix; the guard
cannot safely borrow its mutable encoder. Likewise, a fault on a VTR worker
retains already written sections. Readers recover such unfinished prefixes.
An independent sealer never reuses a backend that unwound through a panic.

The native C++ SDK currently has no crash guard. See the
[C++ guide](../vtr-cpp/README.md) for the native recording API.

`cargo test -p vtr-guard` exercises fatal signals, interrupted payload capture,
process exit, graceful stop, and forced stop in isolated Linux subprocesses.
