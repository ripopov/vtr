// The demo design of the pipeline tracer suite (docs/c910-verilator-tx-stream.html).
//
// demo_core stands in for a small out-of-order core: a script indexed by the cycle
// counter drives the handshake signals such a core's control logic would, so the
// tracer bound to it (tracer.sv, bind.sv) meets every situation of the tracker in
// about a dozen instructions. The design knows nothing about tracing; tb declares
// its clock through the vtr_trace package.
`timescale 1ns / 1ns

module demo_core (
    input logic clk
);
  logic [7:0] cyc = 0;  // edges so far
  always_ff @(posedge clk) cyc <= cyc + 1;
  // At edge a the pipeline registers load what the signals below say during the cycle
  // before it; the script is indexed by that edge.
  wire  [7:0] a = cyc + 1;

  logic if_fire;  // fetch accepts an instruction at if_pc
  logic [15:0] if_pc;
  logic id_load, id_stall;  // decode takes the oldest fetched instruction / cannot
  logic disp_en;  // dispatch: the disp_num oldest decoded instructions share ROB entry disp_idx
  logic [1:0] disp_num, disp_idx;
  logic disp_preg_vld;  // a single dispatched instruction writes physical register disp_preg
  logic [3:0] disp_preg;
  logic ex_en, ex_src_vld, ex_mem;  // ROB entry ex_idx executes; its operand came from ex_src_idx
  logic [1:0] ex_idx, ex_src_idx;
  logic [15:0] ex_addr;  // a memory access issues a bus request tagged with ex_idx
  logic replay_vld;  // ROB entry replay_idx was replayed
  logic [1:0] replay_idx;
  logic cm_preg_vld, cm_rob_vld;  // write-back by physical register / by ROB entry
  logic [3:0] cm_preg;
  logic [1:0] cm_rob_idx;
  logic rt_en, rt_err;  // ROB entry rt_idx retires (with an exception)
  logic [1:0] rt_idx;
  logic br_mispredict;  // the branch in ROB entry br_idx mispredicted
  logic [1:0] br_idx;
  logic flush_fe;  // the front end is flushed
  logic bus_data, bus_resp;  // bus request bus_tag enters its data phase / completes
  logic [1:0] bus_tag;
  logic halt;  // the bus is reset at the end of the test

  always_comb begin
    {if_fire, if_pc, id_load, id_stall, disp_en, disp_num, disp_idx, disp_preg_vld, disp_preg} = '0;
    {ex_en, ex_src_vld, ex_mem, ex_idx, ex_src_idx, ex_addr, replay_vld, replay_idx} = '0;
    {cm_preg_vld, cm_rob_vld, cm_preg, cm_rob_idx, rt_en, rt_err, rt_idx} = '0;
    {br_mispredict, br_idx, flush_fe, bus_data, bus_resp, bus_tag, halt} = '0;
    case (a)
      2: begin if_fire = 1; if_pc = 'h100; end
      3: begin id_load = 1; if_fire = 1; if_pc = 'h104; end
      4: begin
        disp_en = 1; disp_num = 1; disp_idx = 0; disp_preg_vld = 1; disp_preg = 5;
        id_load = 1; if_fire = 1; if_pc = 'h108;
      end
      5: begin
        ex_en = 1; ex_idx = 0;
        disp_en = 1; disp_num = 1; disp_idx = 1; disp_preg_vld = 1; disp_preg = 6;
        id_stall = 1; if_fire = 1; if_pc = 'h10c;
      end
      6: begin
        cm_preg_vld = 1; cm_preg = 5;
        ex_en = 1; ex_idx = 1; ex_src_vld = 1; ex_src_idx = 0; ex_mem = 1; ex_addr = 'h2000;
        id_stall = 1;
      end
      7: begin bus_data = 1; bus_tag = 1; replay_vld = 1; replay_idx = 1; id_load = 1; end
      8: begin
        rt_en = 1; rt_idx = 0; cm_preg_vld = 1; cm_preg = 6; bus_resp = 1; bus_tag = 1;
        id_load = 1; if_fire = 1; if_pc = 'h110;
      end
      9: begin disp_en = 1; disp_num = 2; disp_idx = 2; id_load = 1; if_fire = 1; if_pc = 'h114; end
      10: begin
        rt_en = 1; rt_idx = 1; ex_en = 1; ex_idx = 2;
        disp_en = 1; disp_num = 1; disp_idx = 3; disp_preg_vld = 1; disp_preg = 7;
        id_load = 1; if_fire = 1; if_pc = 'h118;
      end
      11: begin cm_rob_vld = 1; cm_rob_idx = 2; ex_en = 1; ex_idx = 3; id_load = 1; if_fire = 1; if_pc = 'h11c; end
      12: begin cm_preg_vld = 1; cm_preg = 7; br_mispredict = 1; br_idx = 3; if_fire = 1; if_pc = 'h200; end
      13: begin rt_en = 1; rt_idx = 2; id_load = 1; if_fire = 1; if_pc = 'h204; end
      14: begin
        rt_en = 1; rt_idx = 3;
        disp_en = 1; disp_num = 1; disp_idx = 0; disp_preg_vld = 1; disp_preg = 5;
        id_load = 1; if_fire = 1; if_pc = 'h208;
      end
      15: begin ex_en = 1; ex_idx = 0; ex_mem = 1; ex_addr = 'h3000; flush_fe = 1; if_fire = 1; if_pc = 'h300; end
      16: begin cm_rob_vld = 1; cm_rob_idx = 1; cm_preg_vld = 1; cm_preg = 5; id_load = 1; end
      18: begin rt_en = 1; rt_idx = 0; rt_err = 1; halt = 1; end
      default: ;
    endcase
  end
endmodule

module tb;
  import vtr_trace::*;
  logic clk = 0;
  always #5 clk = ~clk;
  demo_core core (.clk);

  // The clock the pipeline is counted in: 10 ns from the first rising edge.
  // verilator tracing_off
  vtr_clock_t c;
  bit on;
  // verilator tracing_on
  initial c = vtr_clock("", "clk");
  always @(posedge clk) if (!on) begin vtr_clock_run(c, 10, VTR_NS); on = 1; end
  initial #200 $finish;
endmodule
