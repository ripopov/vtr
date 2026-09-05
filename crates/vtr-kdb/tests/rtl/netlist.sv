module cell_unit #(parameter W=4)(input logic clk, input logic [W-1:0] a, b,
    output logic [W-1:0] q, spare);
    assign spare = '0;
    always_ff @(posedge clk) q <= a ^ b;
endmodule
module group_unit #(parameter W=4)(input logic clk, input logic [W-1:0] a,
    output logic [W-1:0] q);
    logic [W-1:0] middle;
    cell_unit #(.W(W)) u(.clk(clk), .a(a), .b({W{1'b1}}), .q(middle), .spare());
    assign q = middle + 1'b1;
endmodule
module top(input logic clk, select, input logic [7:0] a, b,
    input logic [95:0] wide, inout wire bus,
    output logic [7:0] q, output logic flag, output logic [95:0] wide_q);
    logic [7:0] g;
    group_unit #(.W(8)) group0(.clk(clk), .a(a), .q(g));
    for (genvar i=0; i<2; i++) begin: lanes
        wire [3:0] unused;
        cell_unit u(.clk(clk),.a(4'b0011),.b(4'b0101),.q(unused),.spare());
    end
    assign q = select ? (g & b) : ~a;
    assign flag = (a == b) || select;
    assign wide_q = wide;
endmodule
