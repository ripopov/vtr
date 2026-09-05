module stage #(parameter W=8)(input logic clk, rst_n, en,
    input logic [W-1:0] d, output logic [W-1:0] q);
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) q <= '0;
        else if (en) q <= d;
    end
endmodule
module top(input logic clk, rst_n, en, sel,
    input logic [7:0] a, b, output logic [7:0] q);
    logic [7:0] mux, first;
    assign mux = sel ? a : b;
    stage u0(.clk(clk), .rst_n(rst_n), .en(en), .d(mux), .q(first));
    stage u1(.clk(clk), .rst_n(rst_n), .en(en), .d(first), .q(q));
endmodule
