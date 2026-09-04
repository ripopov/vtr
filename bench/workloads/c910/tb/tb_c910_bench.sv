// Testbench for the single-core openC910 smart_run SoC, driven by Verilator.
// Derived from ext/pulp-c910/vendor/thead_openc910/smart_run/logical/tb/tb_verilator.v
// (Copyright 2019-2021 T-Head Semiconductor Co., Ltd., Apache-2.0).
//
// Differences w.r.t. the vendor TB: no $dumpvars (tracing is done by the C++
// harness so that FST and VTR are produced by the same Verilator trace code),
// cycle / retired-instruction counters and the run status exported as top-level
// ports, and termination decided in C++ rather than by $finish.

`include "cpu_cfig.h"

`timescale 1ns/100ps

`define SOC_TOP             top.x_soc
`define RTL_MEM             top.x_soc.x_axi_slave128.x_f_spsram_large
`define CPU_TOP             top.x_soc.x_cpu_sub_system_axi.x_rv_integration_platform.x_cpu_top

module top (
  input  wire        clk,
  output reg  [63:0] o_cycles,     // core clock cycles since reset release
  output reg  [63:0] o_instret,    // instructions retired since reset release
  output reg         o_running,    // 1 after reset release, before end of test
  output reg         o_pass,
  output reg         o_fail
);

  reg  jclk;
  reg  rst_b;
  reg  jrst_b;
  wire jtg_tms;
  wire jtg_tdi;
  wire jtg_tdo;
  wire uart0_sin;
  wire [7:0] b_pad_gpio_porta;

  // ---------------------------------------------------------------- clocking
  // JTAG clock: /4 of the core clock (unused by the benchmarks, kept for
  // structural fidelity with the vendor TB).
  integer jclkCnt;
  initial begin
    jclk    = 0;
    jclkCnt = 0;
  end
  always @(posedge clk) begin
    if (jclkCnt < 1) jclkCnt = jclkCnt + 1;
    else begin
      jclkCnt = 0;
      jclk    = !jclk;
    end
  end

  // ------------------------------------------------------------------- reset
  integer rst_bCnt;
  initial begin
    rst_bCnt = 0;
    rst_b    = 1;
  end
  always @(posedge clk) begin
    rst_bCnt = rst_bCnt + 1;
    if (rst_bCnt > 10 && rst_bCnt < 20) rst_b = 0;
    else if (rst_bCnt > 20)             rst_b = 1;
  end

  integer jrstCnt;
  initial begin
    jrst_b  = 1;
    jrstCnt = 0;
  end
  always @(posedge clk) begin
    jrstCnt = jrstCnt + 1;
    if (jrstCnt > 40 && jrstCnt < 80) jrst_b = 0;
    else if (jrstCnt > 80)            jrst_b = 1;
  end

  // ------------------------------------------------------------ program load
  integer i;
  integer j;
  bit [31:0] mem_inst_temp [65536];
  bit [31:0] mem_data_temp [65536];

  initial begin
    $display("[tb] loading inst.pat / data.pat");
    $readmemh("inst.pat", mem_inst_temp);
    $readmemh("data.pat", mem_data_temp);

    i = 0;
    for (j = 0; i < 32'h4000; i = j/4) begin
      `RTL_MEM.ram0.mem[i][7:0]  = mem_inst_temp[j][31:24];
      `RTL_MEM.ram1.mem[i][7:0]  = mem_inst_temp[j][23:16];
      `RTL_MEM.ram2.mem[i][7:0]  = mem_inst_temp[j][15: 8];
      `RTL_MEM.ram3.mem[i][7:0]  = mem_inst_temp[j][ 7: 0];
      j = j+1;
      `RTL_MEM.ram4.mem[i][7:0]  = mem_inst_temp[j][31:24];
      `RTL_MEM.ram5.mem[i][7:0]  = mem_inst_temp[j][23:16];
      `RTL_MEM.ram6.mem[i][7:0]  = mem_inst_temp[j][15: 8];
      `RTL_MEM.ram7.mem[i][7:0]  = mem_inst_temp[j][ 7: 0];
      j = j+1;
      `RTL_MEM.ram8.mem[i][7:0]  = mem_inst_temp[j][31:24];
      `RTL_MEM.ram9.mem[i][7:0]  = mem_inst_temp[j][23:16];
      `RTL_MEM.ram10.mem[i][7:0] = mem_inst_temp[j][15: 8];
      `RTL_MEM.ram11.mem[i][7:0] = mem_inst_temp[j][ 7: 0];
      j = j+1;
      `RTL_MEM.ram12.mem[i][7:0] = mem_inst_temp[j][31:24];
      `RTL_MEM.ram13.mem[i][7:0] = mem_inst_temp[j][23:16];
      `RTL_MEM.ram14.mem[i][7:0] = mem_inst_temp[j][15: 8];
      `RTL_MEM.ram15.mem[i][7:0] = mem_inst_temp[j][ 7: 0];
      j = j+1;
    end

    i = 0;
    for (j = 0; i < 32'h4000; i = j/4) begin
      `RTL_MEM.ram0.mem[i+32'h4000][7:0]  = mem_data_temp[j][31:24];
      `RTL_MEM.ram1.mem[i+32'h4000][7:0]  = mem_data_temp[j][23:16];
      `RTL_MEM.ram2.mem[i+32'h4000][7:0]  = mem_data_temp[j][15: 8];
      `RTL_MEM.ram3.mem[i+32'h4000][7:0]  = mem_data_temp[j][ 7: 0];
      j = j+1;
      `RTL_MEM.ram4.mem[i+32'h4000][7:0]  = mem_data_temp[j][31:24];
      `RTL_MEM.ram5.mem[i+32'h4000][7:0]  = mem_data_temp[j][23:16];
      `RTL_MEM.ram6.mem[i+32'h4000][7:0]  = mem_data_temp[j][15: 8];
      `RTL_MEM.ram7.mem[i+32'h4000][7:0]  = mem_data_temp[j][ 7: 0];
      j = j+1;
      `RTL_MEM.ram8.mem[i+32'h4000][7:0]  = mem_data_temp[j][31:24];
      `RTL_MEM.ram9.mem[i+32'h4000][7:0]  = mem_data_temp[j][23:16];
      `RTL_MEM.ram10.mem[i+32'h4000][7:0] = mem_data_temp[j][15: 8];
      `RTL_MEM.ram11.mem[i+32'h4000][7:0] = mem_data_temp[j][ 7: 0];
      j = j+1;
      `RTL_MEM.ram12.mem[i+32'h4000][7:0] = mem_data_temp[j][31:24];
      `RTL_MEM.ram13.mem[i+32'h4000][7:0] = mem_data_temp[j][23:16];
      `RTL_MEM.ram14.mem[i+32'h4000][7:0] = mem_data_temp[j][15: 8];
      `RTL_MEM.ram15.mem[i+32'h4000][7:0] = mem_data_temp[j][ 7: 0];
      j = j+1;
    end
    $display("[tb] program loaded");
  end

  // --------------------------------------------------- performance counters
  wire       retire0 = `CPU_TOP.core0_pad_retire0;
  wire       retire1 = `CPU_TOP.core0_pad_retire1;
  wire       retire2 = `CPU_TOP.core0_pad_retire2;
  wire [1:0] retire_n = {1'b0, retire0} + {1'b0, retire1} + {1'b0, retire2};

  initial begin
    o_cycles  = 64'd0;
    o_instret = 64'd0;
    o_running = 1'b0;
    o_pass    = 1'b0;
    o_fail    = 1'b0;
  end

  always @(posedge clk) begin
    if (!rst_b) begin
      o_cycles  <= 64'd0;
      o_instret <= 64'd0;
      o_running <= 1'b1;
    end else if (o_running && !o_pass && !o_fail) begin
      o_cycles  <= o_cycles + 64'd1;
      o_instret <= o_instret + {62'd0, retire_n};
    end
  end


  // ---------------------------------------------- end-of-test / console port
  reg [31:0] cpu_awaddr;
  reg [3:0]  cpu_awlen;
  reg [15:0] cpu_wstrb;
  reg        cpu_wvalid;
  reg [63:0] value0;
  reg [63:0] value1;
  reg [63:0] value2;

  always @(posedge clk) begin
    cpu_awlen[3:0]   <= `SOC_TOP.x_axi_slave128.awlen[3:0];
    cpu_awaddr[31:0] <= `SOC_TOP.x_axi_slave128.mem_addr[31:0];
    cpu_wvalid       <= `SOC_TOP.biu_pad_wvalid;
    cpu_wstrb        <= `SOC_TOP.biu_pad_wstrb;
    value0           <= `CPU_TOP.x_ct_top_0.x_ct_core.x_ct_iu_top.x_ct_iu_rbus.rbus_pipe0_wb_data[63:0];
    value1           <= `CPU_TOP.x_ct_top_0.x_ct_core.x_ct_iu_top.x_ct_iu_rbus.rbus_pipe1_wb_data[63:0];
    value2           <= `CPU_TOP.x_ct_top_0.x_ct_core.x_ct_lsu_top.x_ct_lsu_ld_wb.ld_wb_preg_data_sign_extend[63:0];
  end

  always @(posedge clk) begin
    if (value0 == 64'h444333222 || value1 == 64'h444333222 || value2 == 64'h444333222)
      o_pass <= 1'b1;
    else if (value0 == 64'h2382348720 || value1 == 64'h2382348720 || value2 == 64'h2382348720)
      o_fail <= 1'b1;
    else if ((cpu_awlen[3:0] == 4'b0) && (cpu_awaddr[31:0] == 32'h01ff_fff0) &&
             cpu_wvalid && `CPU_TOP.axim_clk_en) begin
      if      (cpu_wstrb[15:0] == 16'h000f) $write("%c", `SOC_TOP.biu_pad_wdata[  7:  0]);
      else if (cpu_wstrb[15:0] == 16'h00f0) $write("%c", `SOC_TOP.biu_pad_wdata[ 39: 32]);
      else if (cpu_wstrb[15:0] == 16'h0f00) $write("%c", `SOC_TOP.biu_pad_wdata[ 71: 64]);
      else if (cpu_wstrb[15:0] == 16'hf000) $write("%c", `SOC_TOP.biu_pad_wdata[103: 96]);
    end
  end

  assign jtg_tdi   = 1'b0;
  assign uart0_sin = 1'b1;

  soc x_soc (
    .i_pad_clk        ( clk              ),
    .b_pad_gpio_porta ( b_pad_gpio_porta ),
    .i_pad_jtg_trst_b ( jrst_b           ),
    .i_pad_jtg_tclk   ( jclk             ),
    .i_pad_jtg_tdi    ( jtg_tdi          ),
    .i_pad_jtg_tms    ( jtg_tms          ),
    .i_pad_uart0_sin  ( uart0_sin        ),
    .o_pad_jtg_tdo    ( jtg_tdo          ),
    .o_pad_uart0_sout ( uart0_sout       ),
    .i_pad_rst_b      ( rst_b            )
  );

endmodule
