// Strata on the ULX3S board (Lattice ECP5): 25 MHz oscillator, HDMI
// connector (DVI), USB serial through the FTDI chip, the configuration SPI
// flash (which also holds the boot image), 7 buttons and 8 LEDs.
module ulx3s_top #(
  parameter ROM_INIT = "build/sw/boot/boot.hex"
) (
  input  logic       clk_25mhz,
  input  logic [6:0] btn,          // [0] PWR (pressed = 0), [1] FIRE1, [2] FIRE2, [3] UP, [4] DOWN, [5] LEFT, [6] RIGHT
  output logic [7:0] led,
  output logic       ftdi_rxd,     // to the host
  input  logic       ftdi_txd,     // from the host
  output logic       flash_csn,
  output logic       flash_mosi,
  input  logic       flash_miso,
  output logic       flash_holdn,
  output logic       flash_wpn,
  output logic [3:0] gpdi_dp,      // HDMI, pseudo-differential (LVCMOS33D)
  output logic       wifi_gpio0    // keeps the ESP32 from resetting the FPGA
);

  logic clk, clk_bit, locked;
  ulx3s_pll u_pll (.clk_in(clk_25mhz), .clk_bit, .clk_px(clk), .locked);

  // Power-on reset: held until the PLL locks and for 2^16 cycles after.
  logic [16:0] por_q = '0;
  always_ff @(posedge clk) begin
    if (!locked) por_q <= '0;
    else if (!por_q[16]) por_q <= por_q + 17'd1;
  end

  // Board buttons to the machine's button bits (docs/memory-map.md, GPIO).
  logic [6:0] buttons;
  assign buttons = {!btn[0], btn[2], btn[1], btn[6], btn[5], btn[4], btn[3]};

  logic spi_sck, halted;
  logic [7:0] halt_code, gpio_led;
  logic [3:0] vid_r, vid_g, vid_b;
  logic vid_hs, vid_vs, vid_de;

  strata_machine #(.ROM_INIT(ROM_INIT)) u_machine (
    .clk, .rst_in(!por_q[16]),
    .uart_rx(ftdi_txd), .uart_tx(ftdi_rxd),
    .btn(buttons), .led(gpio_led),
    .spi_cs_n(flash_csn), .spi_sck, .spi_mosi(flash_mosi), .spi_miso(flash_miso),
    .vid_r, .vid_g, .vid_b, .vid_hs, .vid_vs, .vid_de,
    .halted, .halt_code,
    .trace_valid(), .trace_trap(), .trace_intr(), .trace_cause(), .trace_pc(), .trace_insn(),
    .trace_next_pc(), .trace_mode(), .trace_rd_we(), .trace_rd(), .trace_rd_wdata()
  );

  // The flash clock pin is the configuration clock: reach it through USRMCLK.
  USRMCLK u_usrmclk (.USRMCLKI(spi_sck), .USRMCLKTS(1'b0));
  assign flash_holdn = 1'b1;
  assign flash_wpn   = 1'b1;
  assign wifi_gpio0  = 1'b1;

  assign led = halted ? halt_code : gpio_led;

  st_hdmi_out u_hdmi (
    .clk_px(clk), .clk_bit, .r(vid_r), .g(vid_g), .b(vid_b),
    .hs(vid_hs), .vs(vid_vs), .de(vid_de), .tmds(gpdi_dp)
  );

endmodule
