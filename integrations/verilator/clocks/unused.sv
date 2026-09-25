// A design that names nothing from the vtr_trace package.
module tb (input logic clk, output logic [3:0] count);
  always_ff @(posedge clk) count <= count + 1;
endmodule
