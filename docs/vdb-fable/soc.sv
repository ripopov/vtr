// Demo design for docs/VDB_Fable.html: a small SoC slice with a register
// block, a DMA-style state machine, a valid/ready handshake and a
// parameterised two-stage pipeline instantiated at two widths. Verilated with
// --trace-vtr by build.py, which embeds the recording and the VDB in the page.
package soc_pkg;
  typedef enum logic [1:0] {IDLE = 2'd0, REQ = 2'd1, BUSY = 2'd2, DONE = 2'd3} dma_state_t;
  typedef struct packed {
    logic       start;
    logic       irq_en;
    logic [5:0] burst;
  } ctrl_t;
endpackage

// One register of a synchronous pipeline with a hold enable.
module stage #(parameter W = 8) (
    input  logic         clk,
    input  logic         rst_n,
    input  logic         en,
    input  logic [W-1:0] d,
    output logic [W-1:0] q);
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) q <= '0;
    else if (en) q <= d;
  end
endmodule

// Two stages in a row; the width is a parameter so top can use it twice.
module pipe #(parameter W = 8) (
    input  logic         clk,
    input  logic         rst_n,
    input  logic         en,
    input  logic [W-1:0] d,
    output logic [W-1:0] q);
  logic [W-1:0] mid;
  stage #(.W(W)) s0(.clk(clk), .rst_n(rst_n), .en(en), .d(d),   .q(mid));
  stage #(.W(W)) s1(.clk(clk), .rst_n(rst_n), .en(en), .d(mid), .q(q));
endmodule

// A control/status register block on a tiny write-enable bus.
module csr (
    input  logic        clk,
    input  logic        rst_n,
    input  logic        wen,
    input  logic [3:0]  addr,
    input  logic [7:0]  wdata,
    output logic [7:0]  rdata,
    output soc_pkg::ctrl_t ctrl,
    input  logic        busy,
    input  logic        done,
    input  logic [7:0]  count);
  import soc_pkg::*;
  localparam logic [3:0] CTRL_ADDR   = 4'h0;
  localparam logic [3:0] STATUS_ADDR = 4'h4;
  localparam logic [3:0] COUNT_ADDR  = 4'h8;
  logic [7:0] status;
  assign status = {6'b0, done, busy};
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) ctrl <= '0;
    else if (wen && addr == CTRL_ADDR) ctrl <= ctrl_t'(wdata);
    else if (done) ctrl <= ctrl_t'({1'b0, ctrl.irq_en, ctrl.burst});
  end
  always_comb begin
    if (addr == CTRL_ADDR) rdata = 8'(ctrl);
    else if (addr == STATUS_ADDR) rdata = status;
    else if (addr == COUNT_ADDR) rdata = count;
    else rdata = 8'h00;
  end
endmodule

// The DMA engine: a four-state machine that requests the bus, holds it for
// `burst` beats and reports completion.
module dma (
    input  logic       clk,
    input  logic       rst_n,
    input  soc_pkg::ctrl_t ctrl,
    output logic       req,
    input  logic       gnt,
    output logic       busy,
    output logic       done,
    output logic [7:0] count);
  import soc_pkg::*;
  dma_state_t state, state_n;
  logic [5:0] beats, beats_n;
  logic       last;
  assign last = beats == 6'd0;
  always_comb begin
    state_n = state;
    beats_n = beats;
    if (state == IDLE) begin
      if (ctrl.start) begin state_n = REQ; beats_n = ctrl.burst; end
    end else if (state == REQ) begin
      if (gnt) state_n = BUSY;
    end else if (state == BUSY) begin
      if (last) state_n = DONE;
      else beats_n = beats - 6'd1;
    end else begin
      state_n = IDLE;
    end
  end
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      state <= IDLE;
      beats <= '0;
      count <= '0;
    end else begin
      state <= state_n;
      beats <= beats_n;
      if (state == BUSY) count <= count + 8'd1;
    end
  end
  assign req  = state == REQ;
  assign busy = state == BUSY;
  assign done = state == DONE;
endmodule

// A round-robin-free arbiter: grants the DMA when the CPU is not using the bus.
module arbiter (
    input  logic clk,
    input  logic rst_n,
    input  logic cpu_req,
    input  logic dma_req,
    output logic dma_gnt);
  logic cpu_hold;
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) cpu_hold <= 1'b0;
    else cpu_hold <= cpu_req;
  end
  assign dma_gnt = dma_req && !cpu_req && !cpu_hold;
endmodule

module top (
    input  logic        clk,
    input  logic        rst_n,
    input  logic        wen,
    input  logic [3:0]  addr,
    input  logic [7:0]  wdata,
    output logic [7:0]  rdata,
    input  logic        cpu_req,
    input  logic [7:0]  a,
    input  logic [15:0] w,
    output logic [7:0]  q,
    output logic [15:0] wq,
    output logic        irq);
  import soc_pkg::*;
  ctrl_t      ctrl;
  logic       req, gnt, busy, done;
  logic [7:0] count;
  csr     u_csr(.clk(clk), .rst_n(rst_n), .wen(wen), .addr(addr), .wdata(wdata), .rdata(rdata),
                .ctrl(ctrl), .busy(busy), .done(done), .count(count));
  dma     u_dma(.clk(clk), .rst_n(rst_n), .ctrl(ctrl), .req(req), .gnt(gnt), .busy(busy), .done(done), .count(count));
  arbiter u_arb(.clk(clk), .rst_n(rst_n), .cpu_req(cpu_req), .dma_req(req), .dma_gnt(gnt));
  pipe #(.W(8))  u_pipe8 (.clk(clk), .rst_n(rst_n), .en(busy), .d(a), .q(q));
  pipe #(.W(16)) u_pipe16(.clk(clk), .rst_n(rst_n), .en(busy), .d(w), .q(wq));
  assign irq = done && ctrl.irq_en;
endmodule
