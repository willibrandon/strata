// Whole-machine testbench for event-driven 4-state simulators (iverilog).
//
// Plusargs:
//   +rom=<hex>        boot ROM contents ($readmemh, one 32-bit word per line)
//   +ram=<hex>        RAM preload (test harness shortcut; the real boot path
//                     loads RAM from flash through the boot ROM)
//   +flash=<hex>      SPI flash contents (bytes) at +flash_offset (default 0x200000)
//   +uart_div=<n>     UART bit period used to decode TX (default 25)
//   +uart_in=<text>   bytes to send to the UART RX pin after reset (\n allowed)
//   +timeout=<n>      cycle limit (default 2,000,000)
//   +trace=<file>     write the retirement trace
//   +vcd=<file>       dump waveforms
//
// Exit: prints "HALT <code> <cycles>" when the machine halts; any X on key
// control outputs after reset is reported as an error.
`timescale 1ns/1ps
module tb_machine;

  logic clk = 1'b0;
  logic rst_in = 1'b1;
  logic uart_rx = 1'b1;
  logic [6:0] btn = '0;
  logic uart_tx, spi_cs_n, spi_sck, spi_mosi, spi_miso;
  logic [7:0] led, halt_code;
  logic [3:0] vid_r, vid_g, vid_b;
  logic vid_hs, vid_vs, vid_de, halted;
  logic trace_valid, trace_trap, trace_intr, trace_rd_we;
  logic [4:0] trace_cause, trace_rd;
  logic [31:0] trace_pc, trace_insn, trace_next_pc, trace_rd_wdata;
  logic [1:0] trace_mode;

  strata_machine u_machine (.*);
  spi_flash_model u_flash (.cs_n(spi_cs_n), .sck(spi_sck), .mosi(spi_mosi), .miso(spi_miso));

  always #20 clk = !clk;   // 25 MHz

  longint cycle = 0;
  always @(posedge clk) cycle <= cycle + 1;

  // ---------------------------------------------------------------- setup
  string rom_path, ram_path, trace_path, vcd_path, uart_in;
  int    uart_div, timeout, trace_fd;

  initial begin
    if (!$value$plusargs("uart_div=%d", uart_div)) uart_div = 25;
    if (!$value$plusargs("timeout=%d", timeout)) timeout = 2_000_000;
    if ($value$plusargs("rom=%s", rom_path)) $readmemh(rom_path, u_machine.u_rom.mem);
    if ($value$plusargs("ram=%s", ram_path)) $readmemh(ram_path, u_machine.u_ram.mem);
    trace_fd = 0;
    if ($value$plusargs("trace=%s", trace_path)) trace_fd = $fopen(trace_path, "w");
    if ($value$plusargs("vcd=%s", vcd_path)) begin
      $dumpfile(vcd_path);
      $dumpvars(0, tb_machine);
    end
    repeat (8) @(posedge clk);
    rst_in <= 1'b0;
  end

  // ---------------------------------------------------------------- UART RX stimulus
  task automatic send_byte(input logic [7:0] b);
    uart_rx <= 1'b0;
    repeat (uart_div) @(posedge clk);
    for (int i = 0; i < 8; i++) begin
      uart_rx <= b[i];
      repeat (uart_div) @(posedge clk);
    end
    uart_rx <= 1'b1;
    repeat (uart_div) @(posedge clk);
  endtask

  initial begin
    if ($value$plusargs("uart_in=%s", uart_in)) begin
      repeat (2000) @(posedge clk);
      for (int i = 0; i < uart_in.len(); i++) begin
        if (uart_in[i] == "\\" && i + 1 < uart_in.len() && uart_in[i+1] == "n") begin
          send_byte(8'h0A); i++;
        end else send_byte(uart_in[i]);
      end
    end
  end

  // ---------------------------------------------------------------- UART TX decode
  initial begin
    logic [7:0] b;
    forever begin
      @(negedge uart_tx);
      repeat (uart_div / 2) @(posedge clk);
      if (uart_tx == 1'b0) begin
        for (int i = 0; i < 8; i++) begin
          repeat (uart_div) @(posedge clk);
          b[i] = uart_tx;
        end
        repeat (uart_div) @(posedge clk);
        $write("%c", b);
        $fflush();
      end
    end
  end

  // ---------------------------------------------------------------- trace, X checks, exit
  always @(posedge clk) if (!rst_in && cycle > 12) begin
    if ($isunknown({halted, trace_valid, uart_tx, spi_cs_n, vid_hs, vid_vs, vid_de,
                    u_machine.u_soc.u_core.ibus_cmd_valid, u_machine.u_soc.u_core.dbus_cmd_valid})) begin
      $display("\nERROR: X on control outputs at cycle %0d", cycle);
      $finish;
    end
    if (trace_valid && trace_fd != 0) begin
      if (trace_trap)
        $fdisplay(trace_fd, "%0d %08x %08x trap %0d%s -> %08x", cycle, trace_pc, trace_insn,
                  trace_cause, trace_intr ? "i" : "", trace_next_pc);
      else if (trace_rd_we)
        $fdisplay(trace_fd, "%0d %08x %08x x%0d=%08x -> %08x", cycle, trace_pc, trace_insn,
                  trace_rd, trace_rd_wdata, trace_next_pc);
      else
        $fdisplay(trace_fd, "%0d %08x %08x -> %08x", cycle, trace_pc, trace_insn, trace_next_pc);
    end
    if (halted) begin
      // Let the UART drain: wait for the line to stay idle for two frames.
      for (int idle = 0; idle < uart_div * 20; ) begin
        @(posedge clk);
        idle = uart_tx ? idle + 1 : 0;
      end
      $display("\nHALT %0d %0d", halt_code, cycle);
      if (trace_fd != 0) $fclose(trace_fd);
      $finish;
    end
    if (cycle >= timeout) begin
      $display("\nTIMEOUT %0d", cycle);
      $finish;
    end
  end

endmodule
