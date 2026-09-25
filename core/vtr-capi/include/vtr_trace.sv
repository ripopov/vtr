// vtr_trace.sv - the SystemVerilog side of VTR (docs/vtr_clocks.html).
//
// One package through which testbenches and designs reach VTR. Every function
// body lives in vtr_trace_dpi.hpp, written against a small sink interface; a
// simulator integration supplies the sink (Verilator: --trace-vtr parses this
// package automatically). Package rules:
//   * Names start with vtr_ / VTR_. A handle type is vtr_<kind>_t, its
//     constructor vtr_<kind>(...), an operation vtr_<kind>_<verb>(handle, ...).
//   * Handles are opaque int unsigned values, 0 = none, tagged with their kind.
//   * Constructors are context imports, called once from an initial block;
//     every other function is a plain import.
//   * Calls take effect at the current simulation time in the file's unit;
//     only _at forms take a time, saved earlier with vtr_now(). A duration
//     carries an explicit unit (VTR_PS, ...).
//   * Scope paths: "" is the calling instance, "^" its parent (each further
//     "^." one more level up), "a.b" is below the calling instance, and
//     "$root.top.a" is absolute.
//   * The sink owns the file: nothing here opens, flushes or closes it.
//   * Safe without a recording: declarations made before the file opens are
//     replayed at open, together with each clock's latest run; other calls
//     return at once while no file is open.
//   * Misuse (a handle of the wrong kind, a period that is not a whole number
//     of file units, a run on a running clock) skips the call; the counts go
//     into the file's simulation log as warnings at close.

`ifndef VTR_TRACE_SV_
`define VTR_TRACE_SV_

// verilator lint_off DECLFILENAME
// verilator lint_off TIMESCALEMOD
// The package's constants are not design state: keep them out of waveforms.
// verilator tracing_off
package vtr_trace;
  // ---- Common to every part of the package -------------------------------------------------

  // Time units as powers of ten, the same values $timeunit returns.
  localparam int VTR_S = 0, VTR_MS = -3, VTR_US = -6, VTR_NS = -9, VTR_PS = -12, VTR_FS = -15;

  // The current simulation time in the file's unit; keep it to pass to an _at call later.
  import "DPI-C" function longint unsigned vtr_now();

  // ---- Clocks ------------------------------------------------------------------------------

  typedef int unsigned vtr_clock_t;  // a clock; 0 = none

  // Declare a clock: a CLOCK stream `name` in `scope`. Call once, from an initial block.
  import "DPI-C" context function vtr_clock_t vtr_clock(string scope, string name);

  // Now, which must be a rising edge, begins a stretch: one edge every `period` units of
  // 10^unit s, until vtr_clock_stop. The clock must not be running.
  // Example: vtr_clock_run(c, 334, VTR_PS).
  import "DPI-C" function void vtr_clock_run(vtr_clock_t c, longint unsigned period, int unit);

  // No more edges at the current speed after now: call it when the clock is gated or when
  // its generator's delay changes. The stretch ends at its last edge at or before now.
  import "DPI-C" function void vtr_clock_stop(vtr_clock_t c);
endpackage
// verilator tracing_on
// verilator lint_on TIMESCALEMOD
// verilator lint_on DECLFILENAME

`endif  // VTR_TRACE_SV_
