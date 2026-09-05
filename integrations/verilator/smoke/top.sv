module stage(input logic clk, input logic [7:0] d, output logic [7:0] q);
    always_ff @(posedge clk) q <= d;
endmodule
module top(input logic clk, input logic [7:0] d, output wire [7:0] q);
    stage u(.clk(clk), .d(d), .q(q));
endmodule
