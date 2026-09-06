module top(input logic clk);
  integer fd;
  initial begin
    fd = $fopen("messages.txt", "w");
    $display("boot complete");
    $info("configuration ready");
    $warning("retry pending");
    $fdisplay(fd, "file message");
    $write("fragment ");
    $write("complete\n");
  end
  always @(posedge clk) begin
    $display("cycle at %0t", $time);
    $strobe("settled at %0t", $time);
    if ($time == 5) begin
      if ($test$plusargs("error")) $error("response rejected");
      if ($test$plusargs("fatal")) $fatal(1, "watchdog expired");
      if ($test$plusargs("finish")) $finish;
      if ($test$plusargs("runtime")) $printtimescale;
      if ($test$plusargs("bytes")) $display("raw %c", 8'hff);
    end
  end
  final begin
    $display("simulation complete");
    $fclose(fd);
  end
endmodule
