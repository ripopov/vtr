// vtr_trace.sv - the SystemVerilog side of VTR (docs/vtr_clocks.html,
// docs/c910-verilator-tx-stream.html).
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
//     "^." one more level up), "a.b" is below the calling instance,
//     "$root.top.a" is absolute, and "/TX.core0" is a scope of the file's own
//     beside the design's instance tree (for streams grouped by function).
//   * The sink owns the file: nothing here opens, flushes or closes it.
//   * Safe without a recording: declarations made before the file opens are
//     replayed at open, together with each clock's latest run; other calls
//     return at once while no file is open.
//   * Misuse (a handle of the wrong kind, a period that is not a whole number
//     of file units, a run on a running clock, a key that names no item)
//     skips the call; the counts go into the file's simulation log as warnings
//     at close.

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
  // ---- Trackers ----------------------------------------------------------------------------
  // A tracker records items (instructions, requests) as transactions of one stream and
  // generator. Tracers never see transaction ids: they name items by (key space, key), the
  // identifiers the hardware already has; vtr_track.hpp maps them to open transactions.
  // Calls of one tracker belong in one always block: their order within an edge matters.

  typedef int unsigned     vtr_tracker_t;   // a stream + generator that items are written to; 0 = none
  typedef int unsigned     vtr_keyspace_t;  // a key space of one tracker; 0 = none
  typedef longint unsigned vtr_key_t;       // a hardware identifier: sequence number, ROB index, queue slot...
  // How an item ended: the C API's VTR_TX_STATUS_* codes. (vtr.h's VTR_OK = 0 is a call's
  // success, not a status, hence the TX in these names.)
  localparam byte VTR_TX_OK = 1, VTR_TX_ERROR = 2, VTR_TX_ABORTED = 3;

  // Create a tracker: a stream named `stream` of kind `kind` with one generator `generator`.
  // `scope` follows the scope rule: "^" for a tracer bound into the unit it traces.
  // `clock` names a clock created with vtr_clock, as a path from the stream's scope or $root;
  // it is stored as the stream's vtr.clock. "" records no clock. Call once, from an initial block.
  import "DPI-C" context function vtr_tracker_t vtr_tracker(string scope, string stream, string kind,
                                                            string generator, string clock);

  // Shorthand for a pipeline: vtr_tracker(scope, stream, "PIPELINE", "instruction", clock).
  import "DPI-C" context function vtr_tracker_t vtr_pipeline(string scope, string stream, string clock);

  // Create a key space `name` in tracker `trk`: one more dictionary from key to item.
  // Every item call below names its item by (space, key); the space implies the tracker.
  import "DPI-C" function vtr_keyspace_t vtr_keyspace(vtr_tracker_t trk, string name);

  // ---- Items: create, rename, advance

  // Create an item named (sp, k) and start its first stage, now. If (sp, k) still names an
  // older item, that item is aborted first and the misuse is counted (a missed retirement).
  import "DPI-C" function void vtr_item_open    (vtr_keyspace_t sp, vtr_key_t k, string stage);

  // The same, but the item and its first stage begin at an earlier time t from vtr_now().
  import "DPI-C" function void vtr_item_open_at (vtr_keyspace_t sp, vtr_key_t k, string stage, longint unsigned t);

  // Give the item named (sp, k) one more name (to_sp, to_k), replacing its to_sp name if it
  // has one. Both names stay valid until it ends. to_sp must belong to the same tracker as sp.
  import "DPI-C" function void vtr_item_bind    (vtr_keyspace_t sp, vtr_key_t k, vtr_keyspace_t to_sp, vtr_key_t to_k);

  // Take the n items of space sp that were opened first, remove their sp names, and name them
  // all (to_sp, to_k), as one group. For an in-order stage handing instructions to a ROB entry.
  // Returns how many items moved (fewer than n if sp holds fewer; that is counted).
  import "DPI-C" function int  vtr_item_bind_oldest(vtr_keyspace_t sp, int n, vtr_keyspace_t to_sp, vtr_key_t to_k);

  // Enter stage `stage` on the main lane now; the previous main-lane stage ends now.
  // Entering the stage the item is already in continues it (a valid held for several cycles).
  import "DPI-C" function void vtr_item_stage   (vtr_keyspace_t sp, vtr_key_t k, string stage);

  // The same, but the stage begins at an earlier time t from vtr_now().
  import "DPI-C" function void vtr_item_stage_at(vtr_keyspace_t sp, vtr_key_t k, string stage, longint unsigned t);

  // Enter stage `stage` on the named lane (for example "stall" or "mem"); other lanes are untouched.
  // An empty stage name ends the lane's current stage without starting another.
  import "DPI-C" function void vtr_item_lane    (vtr_keyspace_t sp, vtr_key_t k, string lane, string stage);

  // ---- Items: describe

  // Record a point event `name` now, for example "replay" or "mispredict".
  import "DPI-C" function void vtr_item_event   (vtr_keyspace_t sp, vtr_key_t k, string name);

  // Set attribute `key` to an integer or a string. A later call with the same key replaces
  // the value; attributes are written when the item ends. Strings are interned.
  import "DPI-C" function void vtr_item_attr_u64(vtr_keyspace_t sp, vtr_key_t k, string key, longint unsigned v);
  import "DPI-C" function void vtr_item_attr_str(vtr_keyspace_t sp, vtr_key_t k, string key, string v);

  // Set the item's caption (the reserved vtr.label attribute), for example "0x2c04 lh a6,0(t1)".
  import "DPI-C" function void vtr_item_label   (vtr_keyspace_t sp, vtr_key_t k, string text);

  // ---- Items: end

  // End the item (or every member of the group) named (sp, k) now with `status`, closing its
  // open stages and releasing all of its names.
  import "DPI-C" function void vtr_item_close   (vtr_keyspace_t sp, vtr_key_t k, byte status);

  // Squashes. Each ends the chosen items now with VTR_TX_ABORTED and returns how many it ended.
  // Items opened after the item named (sp, k); that item itself survives. Open order is
  // program order when the tracer opens instructions in program order, as front ends do.
  import "DPI-C" function int  vtr_item_abort_younger(vtr_keyspace_t sp, vtr_key_t k);
  // Every item that still has a name in space sp, for example "everything not yet dispatched".
  import "DPI-C" function int  vtr_keyspace_abort    (vtr_keyspace_t sp);
  // Every open item of the tracker.
  import "DPI-C" function int  vtr_tracker_abort     (vtr_tracker_t trk);

  // ---- Links between items (also across trackers)

  // Record a relation of kind `kind` from item A to item B, for example "wakeup" from a producer
  // to a consumer. Both items must still be named when the call is made.
  import "DPI-C" function void vtr_item_relate(string kind, vtr_keyspace_t sa, vtr_key_t ka,
                                               vtr_keyspace_t sb, vtr_key_t kb);

  // Make item B the parent of item A, for example an instruction as the parent of its bus request.
  import "DPI-C" function void vtr_item_parent(vtr_keyspace_t sa, vtr_key_t ka, vtr_keyspace_t sb, vtr_key_t kb);
endpackage
// verilator tracing_on
// verilator lint_on TIMESCALEMOD
// verilator lint_on DECLFILENAME

`endif  // VTR_TRACE_SV_
