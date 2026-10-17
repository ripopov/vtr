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

The [C API guide](../vtr-capi/README.md) covers linking `libvtr` and using the
guard through `vtr.h`. Rust callers can use the API documented in
[`src/lib.rs`](src/lib.rs).
