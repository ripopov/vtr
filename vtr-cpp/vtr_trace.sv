// vtr_trace.sv - the SystemVerilog side of VTR.

`ifndef VTR_TRACE_SV_
`define VTR_TRACE_SV_

package vtr_trace;
  timeunit 1ps;
  timeprecision 1ps;

  localparam int VTR_S = 0, VTR_MS = -3, VTR_US = -6, VTR_NS = -9, VTR_PS = -12, VTR_FS = -15;

  import "DPI-C" function longint unsigned vtr_now();


  typedef int unsigned vtr_clock_t;

  import "DPI-C" context function vtr_clock_t vtr_clock(string scope, string name);

  import "DPI-C" function void vtr_clock_run(vtr_clock_t c, longint unsigned period, int unit);

  import "DPI-C" function void vtr_clock_stop(vtr_clock_t c);

  typedef int unsigned     vtr_tracker_t;
  typedef int unsigned     vtr_keyspace_t;
  typedef longint unsigned vtr_key_t;
  localparam byte VTR_TX_OK = 1, VTR_TX_ERROR = 2, VTR_TX_ABORTED = 3;

  import "DPI-C" context function vtr_tracker_t vtr_tracker(string scope, string generator, string kind, string clock);

  import "DPI-C" context function vtr_tracker_t vtr_pipeline(string scope, string generator, string clock);

  import "DPI-C" function vtr_keyspace_t vtr_keyspace(vtr_tracker_t trk, string name);


  import "DPI-C" function void vtr_item_open    (vtr_keyspace_t sp, vtr_key_t k, string stage);

  import "DPI-C" function void vtr_item_open_at (vtr_keyspace_t sp, vtr_key_t k, string stage, longint unsigned t);

  import "DPI-C" function void vtr_item_bind    (vtr_keyspace_t sp, vtr_key_t k, vtr_keyspace_t to_sp, vtr_key_t to_k);

  import "DPI-C" function int  vtr_item_bind_oldest(vtr_keyspace_t sp, int n, vtr_keyspace_t to_sp, vtr_key_t to_k);

  import "DPI-C" function void vtr_item_stage   (vtr_keyspace_t sp, vtr_key_t k, string stage);

  import "DPI-C" function void vtr_item_stage_at(vtr_keyspace_t sp, vtr_key_t k, string stage, longint unsigned t);

  import "DPI-C" function void vtr_item_lane    (vtr_keyspace_t sp, vtr_key_t k, string lane, string stage);


  import "DPI-C" function void vtr_item_event   (vtr_keyspace_t sp, vtr_key_t k, string name);

  import "DPI-C" function void vtr_item_attr_u64(vtr_keyspace_t sp, vtr_key_t k, string key, longint unsigned v);
  import "DPI-C" function void vtr_item_attr_str(vtr_keyspace_t sp, vtr_key_t k, string key, string v);

  import "DPI-C" function void vtr_item_label   (vtr_keyspace_t sp, vtr_key_t k, string text);


  import "DPI-C" function void vtr_item_close   (vtr_keyspace_t sp, vtr_key_t k, byte status);

  import "DPI-C" function int  vtr_item_abort_younger(vtr_keyspace_t sp, vtr_key_t k);
  import "DPI-C" function int  vtr_keyspace_abort    (vtr_keyspace_t sp);
  import "DPI-C" function int  vtr_tracker_abort     (vtr_tracker_t trk);


  import "DPI-C" function void vtr_item_relate(string kind, vtr_keyspace_t sa, vtr_key_t ka,
                                               vtr_keyspace_t sb, vtr_key_t kb);

  import "DPI-C" function void vtr_item_open_child(vtr_keyspace_t child_sp, vtr_key_t child_k,
                                                  string stage, vtr_keyspace_t parent_sp,
                                                  vtr_key_t parent_k);

  import "DPI-C" function void vtr_item_open_child_at(vtr_keyspace_t child_sp, vtr_key_t child_k,
                                                     string stage, vtr_keyspace_t parent_sp,
                                                     vtr_key_t parent_k, longint unsigned t);

endpackage

`endif
