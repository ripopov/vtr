// Exercise context sizing, signed arithmetic, selection and source semantics.
module child(input logic [6:0] a, output logic [6:0] q);
    assign q = a + 7'd1;
endmodule
module top(input logic clk, input logic signed [6:0] a, b,
           input logic [2:0] index,
           output logic signed [14:0] sum, difference, product, quotient, remainder,
           output logic [6:0] bits, shift_l, shift_r,
           output logic signed [6:0] shift_s,
           output logic compare, logical, reduction, selected, ascending, offset,
           output logic [6:0] cast_value, hierarchical, enum_value, comb, array_value,
           output logic [14:0] replicated);
    typedef enum logic [6:0] {IDLE=7'd3, ACTIVE=7'd7} state_t;
    state_t state;
    wire [0:6] asc = a;
    wire [9:3] off = a;
    child u(.a(a), .q());
    child array_u[2:1](.a(a), .q());
    assign array_value = array_u[1].q;
    assign sum = a + b;
    assign difference = a - b;
    assign product = a * b;
    assign quotient = a / b;
    assign remainder = a % b;
    assign bits = (a & b) | (a ^ ~b);
    assign shift_l = a << index;
    assign shift_r = a >> index;
    assign shift_s = a >>> index;
    assign compare = a < b;
    assign logical = (a != 0) && (b == 3 || a >= b);
    assign reduction = (&a) | (^b) | !(|a);
    assign selected = a[index];
    assign ascending = asc[index];
    assign offset = off[index + 3];
    assign cast_value = 7'(sum);
    assign hierarchical = top.u.q;
    assign enum_value = state == IDLE ? ACTIVE : IDLE;
    assign replicated = {5{{a[2], a[1], a[0]}}};
    always @* comb = a + 7'd2;
    always_ff @(posedge clk) state <= IDLE;
endmodule
