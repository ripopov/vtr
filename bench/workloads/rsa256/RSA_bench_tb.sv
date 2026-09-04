// Benchmark driver for the RSA256 design from ext/libfstwriter (MIT): feeds a new
// pseudo-random (msg, key, modulus) triple whenever the core accepts input, so the
// datapath stays busy for as many cycles as the C++ harness runs.
parameter MOD_WIDTH = 256;
parameter INT_WIDTH = 32;

typedef logic [MOD_WIDTH-1:0] KeyType;
typedef logic [INT_WIDTH-1:0] IntType;

typedef struct packed {
  IntType power;
  KeyType modulus;
} TwoPowerIn;
typedef KeyType TwoPowerOut;

typedef struct packed {
  KeyType a;
  KeyType b;
  KeyType modulus;
} MontgomeryIn;
typedef KeyType MontgomeryOut;

typedef struct packed {
  KeyType base;
  KeyType msg;
  KeyType key;
  KeyType modulus;
} RSAMontModIn;
typedef KeyType RSAMontModOut;

typedef struct packed {
  KeyType msg;
  KeyType key;
  KeyType modulus;
} RSAModIn;
typedef KeyType RSAModOut;

module Testbench (
    input clk,
    input rst_n
);

logic i_valid;
logic i_ready;
RSAModIn i_in;
logic o_valid;
logic o_ready;
logic [255:0] o_out;
logic [255:0] lfsr;
int unsigned n_in, n_out;

assign o_ready = 1'b1;

always_ff @(posedge clk or negedge rst_n) begin
  if (!rst_n) begin
    i_valid <= 1'b0;
    i_in <= '0;
    lfsr <= 256'h412820616369726641206874756F53202C48544542415A494C452054524F50;
    n_in <= 0;
    n_out <= 0;
  end else begin
    if (o_valid) n_out <= n_out + 1;
    if (!i_valid || i_ready) begin
      // Next pseudo-random operands (256-bit LFSR-ish mixing).
      lfsr <= {lfsr[254:0], lfsr[255] ^ lfsr[250] ^ lfsr[245] ^ lfsr[240]} ^ {8{n_in[31:0]}};
      i_in.msg <= lfsr;
      i_in.key <= 256'h10001 | (lfsr[15:4] << 20);
      i_in.modulus <= 256'hE07122F2A4A9E81141ADE518A2CD7574DCB67060B005E24665EF532E0CCA73E1 ^ {lfsr[63:0], 192'b0};
      i_valid <= 1'b1;
      n_in <= n_in + 1;
    end
  end
end

RSA i_rsa (
  .clk(clk),
  .rst_n(rst_n),
  .i_valid(i_valid),
  .i_ready(i_ready),
  .i_in(i_in),
  .o_valid(o_valid),
  .o_ready(o_ready),
  .o_out(o_out)
);

endmodule
