// Board-level testbench for the ULX3S top: the whole FPGA design (PLL, power-on
// reset, machine, DVI serializer) with the configuration flash, the 25 MHz
// oscillator and a DVI receiver.
//
// The receiver samples the four TMDS lanes at the bit rate, finds word
// boundaries from the clock lane, and decodes each word as a DVI sink would
// (control tokens and transition-minimized data). Every decoded word is
// checked against the machine's own video outputs (vid_*) in order, starting
// at the first horizontal sync, so the check covers every pixel and sync
// period of every frame. Decoded frames are also assembled with the same
// rule as the simulator's display model (strata-sim devices.rs) so they can
// be compared with the gate-level and RTL models.
//
// Plusargs:
//   +flash=<hex>   configuration flash contents at 0x200000 (bytes)
//   +frame=<n>     write decoded frame n to +ppm=<file> and finish
//   +timeout=<n>   pixel-clock cycle limit after reset (default 4,000,000)
//
// Prints the UART output, then "FRAME <n> WORDS <w> MISMATCHES <m> ..." and
// "PASS" or "FAIL".
`timescale 1ns/1ps
module tb_ulx3s;

  logic clk_25mhz = 1'b0;
  always #20 clk_25mhz = !clk_25mhz;

  logic [6:0] btn = 7'b0000001;   // PWR (btn[0]) is active low
  logic [7:0] led;
  logic ftdi_rxd, flash_csn, flash_mosi, flash_miso, flash_holdn, flash_wpn, wifi_gpio0;
  logic [3:0] gpdi_dp;

  ulx3s_top dut (
    .clk_25mhz, .btn, .led, .ftdi_rxd, .ftdi_txd(1'b1),
    .flash_csn, .flash_mosi, .flash_miso, .flash_holdn, .flash_wpn,
    .gpdi_dp, .wifi_gpio0
  );

  spi_flash_model u_flash (.cs_n(flash_csn), .sck(dut.spi_sck), .mosi(flash_mosi), .miso(flash_miso));

  int target_frame, timeout;
  string ppm_path;
  initial begin
    if (!$value$plusargs("frame=%d", target_frame)) target_frame = 6;
    if (!$value$plusargs("ppm=%s", ppm_path)) ppm_path = "frame.ppm";
    if (!$value$plusargs("timeout=%d", timeout)) timeout = 4_000_000;
  end

  // ---------------------------------------------------------------- UART
  localparam int UART_DIV = 25;
  initial begin
    logic [7:0] b;
    forever begin
      @(negedge ftdi_rxd);
      repeat (UART_DIV / 2) @(posedge dut.clk);
      if (ftdi_rxd == 1'b0) begin
        for (int i = 0; i < 8; i++) begin
          repeat (UART_DIV) @(posedge dut.clk);
          b[i] = ftdi_rxd;
        end
        repeat (UART_DIV) @(posedge dut.clk);
        $write("%c", b);
        $fflush();
      end
    end
  end

  // ---------------------------------------------------------------- source stream
  // What the machine presents to the serializer, one entry per pixel clock.
  typedef struct packed {
    logic       de;
    logic [3:0] r, g, b;
    logic       hs, vs;
  } px_t;

  px_t  src_q[$];
  bit   src_started = 1'b0, src_seen_idle = 1'b0;
  longint cycles = 0;

  always @(posedge dut.clk) begin
    if (dut.por_q[16]) begin
      px_t p;
      p = '{de: dut.vid_de, r: dut.vid_r, g: dut.vid_g, b: dut.vid_b, hs: dut.vid_hs, vs: dut.vid_vs};
      cycles++;
      if (!src_started && !p.hs && src_seen_idle) src_started = 1'b1;
      if (p.hs) src_seen_idle = 1'b1;
      if (src_started) src_q.push_back(p);
      if (cycles > timeout) begin
        $display("\nTIMEOUT after %0d cycles", cycles);
        $display("FAIL");
        $finish;
      end
    end
  end

  // ---------------------------------------------------------------- DVI receiver
  // Bits are sampled 1 ns after each bit-clock edge: the rising edge starts
  // an even bit (D0), the falling edge an odd one (D1).
  logic [9:0] acc [4];
  int         nbits = 0;
  logic       last_clk_bit = 1'b0;
  bit         rx_started = 1'b0, rx_seen_idle = 1'b0;
  longint     words = 0, mismatches = 0;
  int         disparity [3] = '{0, 0, 0};
  int         max_disparity = 0;

  function automatic logic [7:0] tmds_decode(input logic [9:0] q);
    logic [7:0] d, o;
    d = q[9] ? ~q[7:0] : q[7:0];
    o[0] = d[0];
    for (int i = 1; i < 8; i++) o[i] = q[8] ? (d[i] ^ d[i-1]) : !(d[i] ^ d[i-1]);
    return o;
  endfunction

  function automatic int control_of(input logic [9:0] q);   // -1 if not a control token
    case (q)
      10'b1101010100: return 0;
      10'b0010101011: return 1;
      10'b0101010100: return 2;
      10'b1010101011: return 3;
      default:        return -1;
    endcase
  endfunction

  function automatic int ones(input logic [9:0] q);
    int n = 0;
    for (int i = 0; i < 10; i++) n += q[i];
    return n;
  endfunction

  // Display model state (same rule as strata-sim's Display::observe).
  logic [11:0] cur [640*480];
  int  fx = 0, fy = 0, frames = 0;
  bit  last_de = 1'b0, last_vs = 1'b1, saw_line = 1'b0;

  task automatic write_frame();
    int fd;
    fd = $fopen(ppm_path, "w");
    $fwrite(fd, "P3\n640 480\n255\n");
    for (int i = 0; i < 640 * 480; i++)
      $fwrite(fd, "%0d %0d %0d\n", {cur[i][11:8], cur[i][11:8]}, {cur[i][7:4], cur[i][7:4]}, {cur[i][3:0], cur[i][3:0]});
    $fclose(fd);
  endtask

  task automatic observe(input bit de, input logic [11:0] rgb, input bit vs);
    if (de) begin
      if (fx < 640 && fy < 480) cur[fy * 640 + fx] = rgb;
      fx++;
      saw_line = 1'b1;
    end else if (last_de) begin
      fx = 0;
      fy++;
    end
    if (last_vs && !vs) begin
      if (saw_line) begin
        frames++;
        if (frames == target_frame) begin
          write_frame();
          $display("\nFRAME %0d WORDS %0d MISMATCHES %0d MAX_DISPARITY %0d CYCLES %0d",
                   frames, words, mismatches, max_disparity, cycles);
          if (mismatches == 0 && words > 0) $display("PASS");
          else $display("FAIL");
          $finish;
        end
      end
      fx = 0;
      fy = 0;
      saw_line = 1'b0;
    end
    last_de = de;
    last_vs = vs;
  endtask

  task automatic receive_word(input logic [9:0] w [4]);
    int c [3];
    bit de, hs, vs;
    logic [7:0] d [3];
    logic [11:0] rgb;
    px_t want;
    for (int i = 0; i < 3; i++) begin
      c[i] = control_of(w[i]);
      d[i] = tmds_decode(w[i]);
    end
    de = (c[0] < 0);
    if (!de) begin
      hs = c[0][0];
      vs = c[0][1];
      for (int i = 0; i < 3; i++) disparity[i] = 0;
    end else begin
      hs = 1'b1;
      vs = 1'b1;
      for (int i = 0; i < 3; i++) begin
        disparity[i] += 2 * ones(w[i]) - 10;
        if (disparity[i] > max_disparity) max_disparity = disparity[i];
        if (-disparity[i] > max_disparity) max_disparity = -disparity[i];
      end
    end
    rgb = {d[2][7:4], d[1][7:4], d[0][7:4]};

    // Start at the first hsync (a 1 -> 0 transition), as the source stream
    // does; the words before it are the reset state (and, before the first
    // pixel clock edge, the flip-flops' power-on contents).
    if (!rx_started) begin
      if (!de && hs) rx_seen_idle = 1'b1;
      if (de || hs || !rx_seen_idle) return;
      rx_started = 1'b1;
    end
    words++;
    if (src_q.size() == 0) begin
      mismatches++;
      if (mismatches <= 5) $display("\nERROR: decoded word %0d has no source pixel", words);
      return;
    end
    want = src_q.pop_front();
    if (de != want.de
        || (de && (rgb != {want.r, want.g, want.b}
                   || d[2][3:0] != d[2][7:4] || d[1][3:0] != d[1][7:4] || d[0][3:0] != d[0][7:4]))
        || (!de && (hs != want.hs || vs != want.vs || c[1] != 0 || c[2] != 0))) begin
      mismatches++;
      if (mismatches <= 5)
        $display("\nERROR: word %0d: decoded de=%0d rgb=%03x hs=%0d vs=%0d (c %0d %0d %0d), source de=%0d rgb=%03x hs=%0d vs=%0d",
                 words, de, rgb, hs, vs, c[0], c[1], c[2], want.de, {want.r, want.g, want.b}, want.hs, want.vs);
    end
    observe(de, rgb, vs);
  endtask

  task automatic sample_bit();
    logic [9:0] w [4];
    // A 0 -> 1 transition on the clock lane starts a word.
    if (gpdi_dp[3] && !last_clk_bit) begin
      if (nbits == 10) begin
        for (int i = 0; i < 4; i++) w[i] = acc[i];
        receive_word(w);
      end
      nbits = 0;
    end
    last_clk_bit = gpdi_dp[3];
    if (nbits < 10) begin
      for (int i = 0; i < 4; i++) acc[i][nbits] = gpdi_dp[i];
      nbits++;
    end
  endtask

  always @(posedge dut.clk_bit) begin #1; sample_bit(); end
  always @(negedge dut.clk_bit) begin #1; sample_bit(); end

endmodule
