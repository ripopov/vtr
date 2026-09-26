// Pipeline tracer for demo_core (core.sv), attached by bind.sv. Every port is a probe
// of an existing demo_core signal; the tracer turns them into vtr_trace calls. It
// names instructions by the identifiers the core has: a fetch sequence number it
// counts (space fe), the ROB entry (rob) and the destination physical register
// (preg); bus requests by the ROB entry that issued them (req).
`timescale 1ns / 1ns

// Keys are the hardware's own identifiers, narrower than vtr_key_t: widen them silently.
// verilator lint_off WIDTHEXPAND
// verilator tracing_off
module demo_tracer (
    input logic clk,
    input logic if_fire, input logic [15:0] if_pc,
    input logic id_load, id_stall,
    input logic disp_en, input logic [1:0] disp_num, disp_idx,
    input logic disp_preg_vld, input logic [3:0] disp_preg,
    input logic ex_en, ex_src_vld, ex_mem, input logic [1:0] ex_idx, ex_src_idx, input logic [15:0] ex_addr,
    input logic replay_vld, input logic [1:0] replay_idx,
    input logic cm_preg_vld, cm_rob_vld, input logic [3:0] cm_preg, input logic [1:0] cm_rob_idx,
    input logic rt_en, rt_err, input logic [1:0] rt_idx,
    input logic br_mispredict, input logic [1:0] br_idx,
    input logic flush_fe,
    input logic bus_data, bus_resp, input logic [1:0] bus_tag,
    input logic halt
);
  import vtr_trace::*;
  vtr_tracker_t  p, bus;
  vtr_keyspace_t fe, rob, preg, req;
  vtr_key_t      n = 0;  // last fetched sequence number
  vtr_key_t      n_dec = 0;  // last decoded sequence number
  bit            stalled = 0;
  longint unsigned t_prev = 0;  // the previous edge

  // The disassembly hook: a caption and a unit per static instruction.
  function automatic string text(logic [15:0] pc);
    case (pc)
      'h100: return "add a0,a0,a1";
      'h104: return "lw a2,0(a0)";
      'h108: return "add a3,a2,a1";
      'h10c: return "sub a4,a3,a0";
      'h110: return "beq a4,zero,0x200";
      'h200: return "xor a5,a5,a5";
      'h204: return "lw a6,8(a0)";
      'h300: return "fence.i";
      default: return "addi a7,a7,1";
    endcase
  endfunction

  initial begin
    p    = vtr_pipeline("^", "pipeline", "$root.tb.clk");  // in demo_core; tb declares clk
    fe   = vtr_keyspace(p, "fe");
    rob  = vtr_keyspace(p, "rob");
    preg = vtr_keyspace(p, "preg");
    bus  = vtr_tracker("^", "bus", "BUS", "request", "$root.tb.clk");
    req  = vtr_keyspace(bus, "req");
  end

  always @(posedge clk) begin  // oldest pipeline position first
    // Retire: the entry sat in the retire stage during the previous cycle.
    if (rt_en) begin
      vtr_item_stage_at(rob, rt_idx, "R", t_prev);
      vtr_item_close(rob, rt_idx, rt_err ? VTR_TX_ERROR : VTR_TX_OK);
    end
    // Write-back.
    if (cm_preg_vld) vtr_item_stage(preg, cm_preg, "C");
    if (cm_rob_vld) vtr_item_stage(rob, cm_rob_idx, "C");
    // Squashes: everything opened after the branch; everything not yet dispatched.
    if (br_mispredict) begin
      void'(vtr_item_abort_younger(rob, br_idx));
      n_dec = n;
    end
    if (flush_fe) begin
      void'(vtr_keyspace_abort(fe));
      n_dec = n;
    end
    // Bus requests, tagged with the ROB entry of the access.
    if (bus_data) vtr_item_stage(req, bus_tag, "D");
    if (bus_resp) vtr_item_close(req, bus_tag, VTR_TX_OK);
    if (halt) void'(vtr_tracker_abort(bus));
    // Execute.
    if (replay_vld) vtr_item_event(rob, replay_idx, "replay");
    if (ex_en) begin
      vtr_item_stage(rob, ex_idx, "X");
      if (ex_src_vld) vtr_item_relate("wakeup", rob, ex_src_idx, rob, ex_idx);
      if (ex_mem) begin
        vtr_item_open_at(req, ex_idx, "A", t_prev);  // the address phase began a cycle ago
        vtr_item_parent(req, ex_idx, rob, ex_idx);
        vtr_item_attr_u64(req, ex_idx, "addr", ex_addr);
      end
    end
    // Dispatch: the oldest decoded instructions share one ROB entry.
    if (disp_en) begin
      void'(vtr_item_bind_oldest(fe, disp_num, rob, disp_idx));
      vtr_item_stage(rob, disp_idx, "Q");
      if (disp_preg_vld) vtr_item_bind(rob, disp_idx, preg, disp_preg);
    end
    // Decode, and a stall lane on the instruction waiting for it.
    if (stalled && !id_stall) vtr_item_lane(fe, n_dec + 1, "stall", "");
    if (id_stall && !stalled) vtr_item_lane(fe, n_dec + 1, "stall", "S");
    stalled = id_stall;
    if (id_load) begin
      n_dec++;
      vtr_item_stage(fe, n_dec, "D");
    end
    // Fetch.
    if (if_fire) begin
      n++;
      vtr_item_open(fe, n, "F");
      vtr_item_attr_u64(fe, n, "pc", if_pc);
      vtr_item_attr_str(fe, n, "unit", text(if_pc).substr(0, 1) == "lw" ? "lsu" : "alu");
      vtr_item_label(fe, n, $sformatf("%04h %s", if_pc, text(if_pc)));
    end
    t_prev = vtr_now();
  end
endmodule
// verilator tracing_on
// verilator lint_on WIDTHEXPAND
