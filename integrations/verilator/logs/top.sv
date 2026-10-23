module top(input logic clk, input logic [2:0] mode);
   timeunit 1ps;
   timeprecision 1ps;
   initial begin
      $display("boot complete");
      $info("initial info");
      $warning("retry pending");
      $write("fragment ");
      $write("complete\n");
   end
   always @(posedge clk) begin
      $display("step %0t", $time);
      if ($time == 5) begin
         case (mode)
           1: $error("response rejected");
           2: $fatal(1, "watchdog expired");
           3: $finish;
           4: $write("raw %c\n", 8'hff);
           5: $printtimescale(top);
           default: ;
         endcase
      end
   end
endmodule
