// Clocks declared through the vtr_trace package (docs/vtr_clocks.html).
`timescale 1ps / 1ps

// A DVFS clock generator. It declares its clock in the parent's scope, next to the
// net it drives, stops it where the delay changes and runs it again at the next
// rising edge.
module gen (
    output logic clk
);
  import vtr_trace::*;
  int unsigned half = 167;
  bit restart = 1;
  vtr_clock_t c;
  initial begin
    clk = 0;
    c = vtr_clock("^", "core_clk");
  end
  // verilator lint_off ZERODLY
  always #(half) clk = ~clk;
  // verilator lint_on ZERODLY
  task automatic set_half(int unsigned h);
    vtr_clock_stop(c);
    half = h;
    restart = 1;
  endtask
  always @(posedge clk)
    if (restart) begin
      vtr_clock_run(c, 2 * half, VTR_PS);
      restart = 0;
    end
endmodule

module tb;
  import vtr_trace::*;
  logic core_clk;
  gen g (.clk(core_clk));

  // A gated bus clock with its period in nanoseconds.
  logic bus_clk = 0;
  bit bus_en = 1;
  bit bus_restart = 1;
  vtr_clock_t bc;
  always #1000 bus_clk = bus_en ? ~bus_clk : 1'b0;
  always @(posedge bus_clk)
    if (bus_restart) begin
      vtr_clock_run(bc, 2, VTR_NS);
      bus_restart = 0;
    end

  // A clock whose period is not a whole number of file units, named by an absolute path.
  vtr_clock_t oc;

  initial begin
    bc = vtr_clock("", "bus_clk");
    oc = vtr_clock("$root.tb.g", "odd_clk");
    vtr_clock_run(oc, 1500, VTR_FS);  // 1.5 ps: reported, not recorded
    #20000 g.set_half(250);  // slower
    #10500 begin  // gate the bus clock off, and misuse a running clock on the way
      vtr_clock_run(g.c, 100, VTR_PS);  // core_clk is running: reported, skipped
      vtr_clock_stop(bc);
      bus_en = 0;
      bus_restart = 1;
    end
    #9000 bus_en = 1;
    #10000 g.set_half(100);  // faster
    #10000 g.set_half(333);
    #10000 $finish;
  end
endmodule
