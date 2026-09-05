module pad(inout wire io, input logic drive, output wire sample);
    assign io = drive ? 1'b1 : 1'bz;
    assign sample = io;
endmodule
module vacant;
endmodule
module top(input logic clk, input logic [7:0] a, input logic [1:0] sel,
           inout wire bus, output logic [7:0] q, output logic flag);
    wire escaped_net;
    wire \<&name  = a[0];
    logic [7:0] state;
    wire [3:0] slice = a[5:2];
    always_ff @(posedge clk) state <= state + 8'd1;
    always_comb case(sel)
       0: q = a;
       default: q = state;
    endcase
    pad u(.io(bus),.drive(a[0]),.sample(flag));
    vacant empty();
endmodule
