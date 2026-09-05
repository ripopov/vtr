module top(input logic clk, rst, en, sel,
           input logic [7:0] a, b,
           output logic [7:0] y, q, z, ordered);
    logic [7:0] tmp;
    always_comb begin
        tmp = a + 8'd1;
        y = tmp;
        if (sel) y = b;
    end
    always_ff @(posedge clk) begin
        if (rst) q <= 0;
        else if (en) q <= y;
    end
    always_ff @(posedge clk) begin
        ordered <= a;
        ordered <= b;
    end
    assign z = {a[3], b[2], 6'b0};
endmodule
