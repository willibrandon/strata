// strata_soc: all of the Strata machine's logic.
//
// This module is the boundary of the gate-level model. Its ports are exactly
//   * the machine pins (docs/memory-map.md#machine-pins),
//   * the ports of the four memory macros (rom, ram, vram, font), which are
//     instantiated outside this module (strata_machine for RTL simulation and
//     FPGA; the simulator's memory models in gate mode), and
//   * the retirement trace, an observation-only output.
// See docs/abstractions.md.
module strata_soc (
  input  logic        clk,
  input  logic        rst_in,

  input  logic        uart_rx,
  output logic        uart_tx,
  input  logic [6:0]  btn,
  output logic [7:0]  led,
  output logic        spi_cs_n,
  output logic        spi_sck,
  output logic        spi_mosi,
  input  logic        spi_miso,
  output logic [3:0]  vid_r,
  output logic [3:0]  vid_g,
  output logic [3:0]  vid_b,
  output logic        vid_hs,
  output logic        vid_vs,
  output logic        vid_de,
  output logic        halted,
  output logic [7:0]  halt_code,

  // Boot ROM: port A instruction fetch, port B data reads.
  output logic        rom_a_en,
  output logic [9:0]  rom_a_addr,
  input  logic [31:0] rom_a_rdata,
  output logic        rom_b_en,
  output logic [9:0]  rom_b_addr,
  input  logic [31:0] rom_b_rdata,
  // RAM: port A instruction fetch, port B data.
  output logic        ram_a_en,
  output logic [13:0] ram_a_addr,
  input  logic [31:0] ram_a_rdata,
  output logic        ram_b_en,
  output logic [3:0]  ram_b_we,
  output logic [13:0] ram_b_addr,
  output logic [31:0] ram_b_wdata,
  input  logic [31:0] ram_b_rdata,
  // Text VRAM: port A data, port B video scan-out.
  output logic        vram_a_en,
  output logic [3:0]  vram_a_we,
  output logic [10:0] vram_a_addr,
  output logic [31:0] vram_a_wdata,
  input  logic [31:0] vram_a_rdata,
  output logic        vram_b_en,
  output logic [10:0] vram_b_addr,
  input  logic [31:0] vram_b_rdata,
  // Font RAM: port A data, port B video scan-out.
  output logic        font_a_en,
  output logic [3:0]  font_a_we,
  output logic [9:0]  font_a_addr,
  output logic [31:0] font_a_wdata,
  input  logic [31:0] font_a_rdata,
  output logic        font_b_en,
  output logic [9:0]  font_b_addr,
  input  logic [31:0] font_b_rdata,

  // Retirement trace (observation only).
  output logic        trace_valid,
  output logic        trace_trap,
  output logic        trace_intr,
  output logic [4:0]  trace_cause,
  output logic [31:0] trace_pc,
  output logic [31:0] trace_insn,
  output logic [31:0] trace_next_pc,
  output logic [1:0]  trace_mode,
  output logic        trace_rd_we,
  output logic [4:0]  trace_rd,
  output logic [31:0] trace_rd_wdata
);

  localparam int NDEV = 7;
  localparam int DEV_SYS = 0, DEV_UART = 1, DEV_TIMER = 2, DEV_GPIO = 3,
                 DEV_SPI = 4, DEV_VIDEO = 5, DEV_INTC = 6;

  logic rst, soft_reset;

  st_reset u_reset (.clk, .rst_in, .soft_req(soft_reset), .rst);

  // ------------------------------------------------------------ CPU
  logic        ibus_cmd_valid, ibus_cmd_ready, ibus_rsp_valid, ibus_rsp_err;
  logic [31:0] ibus_cmd_addr, ibus_rsp_rdata;
  logic        dbus_cmd_valid, dbus_cmd_we, dbus_cmd_ready, dbus_rsp_valid, dbus_rsp_err;
  logic [31:0] dbus_cmd_addr, dbus_cmd_wdata, dbus_rsp_rdata;
  logic [3:0]  dbus_cmd_be;
  logic [63:0] mtime;
  logic        msip, mtip, meip;

  st_core u_core (
    .clk, .rst, .halt(halted),
    .mtime, .irq_msip(msip), .irq_mtip(mtip), .irq_meip(meip),
    .ibus_cmd_valid, .ibus_cmd_addr, .ibus_cmd_ready,
    .ibus_rsp_valid, .ibus_rsp_rdata, .ibus_rsp_err,
    .dbus_cmd_valid, .dbus_cmd_addr, .dbus_cmd_we, .dbus_cmd_be, .dbus_cmd_wdata, .dbus_cmd_ready,
    .dbus_rsp_valid, .dbus_rsp_rdata, .dbus_rsp_err,
    .trace_valid, .trace_trap, .trace_intr, .trace_cause, .trace_pc, .trace_insn,
    .trace_next_pc, .trace_mode, .trace_rd_we, .trace_rd, .trace_rd_wdata
  );

  // ------------------------------------------------------------ interconnect
  st_ibus u_ibus (
    .clk, .rst,
    .cmd_valid(ibus_cmd_valid), .cmd_addr(ibus_cmd_addr), .cmd_ready(ibus_cmd_ready),
    .rsp_valid(ibus_rsp_valid), .rsp_rdata(ibus_rsp_rdata), .rsp_err(ibus_rsp_err),
    .rom_en(rom_a_en), .rom_addr(rom_a_addr), .rom_rdata(rom_a_rdata),
    .ram_en(ram_a_en), .ram_addr(ram_a_addr), .ram_rdata(ram_a_rdata)
  );

  logic [NDEV-1:0]    dev_req;
  logic               dev_we;
  logic [9:0]         dev_addr;
  logic [31:0]        dev_wdata;
  logic [NDEV*32-1:0] dev_rdata;

  st_dbus #(.NDEV(NDEV)) u_dbus (
    .clk, .rst,
    .cmd_valid(dbus_cmd_valid), .cmd_addr(dbus_cmd_addr), .cmd_we(dbus_cmd_we),
    .cmd_be(dbus_cmd_be), .cmd_wdata(dbus_cmd_wdata), .cmd_ready(dbus_cmd_ready),
    .rsp_valid(dbus_rsp_valid), .rsp_rdata(dbus_rsp_rdata), .rsp_err(dbus_rsp_err),
    .rom_en(rom_b_en), .rom_addr(rom_b_addr), .rom_rdata(rom_b_rdata),
    .ram_en(ram_b_en), .ram_we(ram_b_we), .ram_addr(ram_b_addr), .ram_wdata(ram_b_wdata),
    .ram_rdata(ram_b_rdata),
    .vram_en(vram_a_en), .vram_we(vram_a_we), .vram_addr(vram_a_addr), .vram_wdata(vram_a_wdata),
    .vram_rdata(vram_a_rdata),
    .font_en(font_a_en), .font_we(font_a_we), .font_addr(font_a_addr), .font_wdata(font_a_wdata),
    .font_rdata(font_a_rdata),
    .dev_req, .dev_we, .dev_addr, .dev_wdata, .dev_rdata
  );

  // ------------------------------------------------------------ devices
  logic uart_irq, gpio_irq, spi_irq, video_irq;

  st_sys u_sys (
    .clk, .rst, .req(dev_req[DEV_SYS]), .we(dev_we), .addr(dev_addr), .wdata(dev_wdata),
    .rdata(dev_rdata[32*DEV_SYS +: 32]), .halted, .halt_code, .reset_req(soft_reset)
  );

  st_uart u_uart (
    .clk, .rst, .req(dev_req[DEV_UART]), .we(dev_we), .addr(dev_addr), .wdata(dev_wdata),
    .rdata(dev_rdata[32*DEV_UART +: 32]), .irq(uart_irq), .rx(uart_rx), .tx(uart_tx)
  );

  st_timer u_timer (
    .clk, .rst, .req(dev_req[DEV_TIMER]), .we(dev_we), .addr(dev_addr), .wdata(dev_wdata),
    .rdata(dev_rdata[32*DEV_TIMER +: 32]), .mtime, .mtip, .msip
  );

  st_gpio u_gpio (
    .clk, .rst, .req(dev_req[DEV_GPIO]), .we(dev_we), .addr(dev_addr), .wdata(dev_wdata),
    .rdata(dev_rdata[32*DEV_GPIO +: 32]), .irq(gpio_irq), .btn, .led
  );

  st_spi u_spi (
    .clk, .rst, .req(dev_req[DEV_SPI]), .we(dev_we), .addr(dev_addr), .wdata(dev_wdata),
    .rdata(dev_rdata[32*DEV_SPI +: 32]), .irq(spi_irq),
    .cs_n(spi_cs_n), .sck(spi_sck), .mosi(spi_mosi), .miso(spi_miso)
  );

  st_video u_video (
    .clk, .rst, .req(dev_req[DEV_VIDEO]), .we(dev_we), .addr(dev_addr), .wdata(dev_wdata),
    .rdata(dev_rdata[32*DEV_VIDEO +: 32]), .irq(video_irq),
    .vram_en(vram_b_en), .vram_addr(vram_b_addr), .vram_rdata(vram_b_rdata),
    .font_en(font_b_en), .font_addr(font_b_addr), .font_rdata(font_b_rdata),
    .vid_r, .vid_g, .vid_b, .vid_hs, .vid_vs, .vid_de
  );

  st_intc #(.N(4)) u_intc (
    .clk, .rst, .req(dev_req[DEV_INTC]), .we(dev_we), .addr(dev_addr), .wdata(dev_wdata),
    .rdata(dev_rdata[32*DEV_INTC +: 32]),
    .sources({spi_irq, video_irq, gpio_irq, uart_irq}), .meip
  );

endmodule
