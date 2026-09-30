// A design that calls into C every cycle, where the simulation can be made to
// die in chosen ways (main.cpp), plus $fatal from the design itself.
module top(input logic clk);
  import "DPI-C" function void maybe_crash(input int cycle);
  logic [31:0] count = 0;
  logic [7:0] lfsr = 8'h1;
  logic [63:0] wide = 0;
  // verilator tracing_off
  int unsigned fatal_at;  // set by +fatal, so not part of the compared trace
  // verilator tracing_on
  initial if (!$value$plusargs("fatal=%d", fatal_at)) fatal_at = 32'hffffffff;
  always @(posedge clk) begin
    count <= count + 1;
    lfsr <= {lfsr[6:0], lfsr[7] ^ lfsr[5] ^ lfsr[4] ^ lfsr[3]};
    wide <= wide + 64'h9e3779b97f4a7c15;
    if (count % 64 == 0) $display("cycle %0d lfsr %h", count, lfsr);
    if (count == fatal_at) $fatal(1, "watchdog expired at cycle %0d", count);
    maybe_crash(count);
  end
endmodule
