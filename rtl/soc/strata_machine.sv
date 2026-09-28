// strata_machine: strata_soc plus behavioral models of its four memory
// macros. Used for RTL simulation and as the core of the FPGA top level.
module strata_machine #(
  parameter ROM_INIT = "",
  parameter RAM_INIT = ""
) (
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

  logic        rom_a_en, rom_b_en, ram_a_en, ram_b_en, vram_a_en, vram_b_en, font_a_en, font_b_en;
  logic [9:0]  rom_a_addr, rom_b_addr, font_a_addr, font_b_addr;
  logic [13:0] ram_a_addr, ram_b_addr;
  logic [10:0] vram_a_addr, vram_b_addr;
  logic [3:0]  ram_b_we, vram_a_we, font_a_we;
  logic [31:0] ram_b_wdata, vram_a_wdata, font_a_wdata;
  logic [31:0] rom_a_rdata, rom_b_rdata, ram_a_rdata, ram_b_rdata;
  logic [31:0] vram_a_rdata, vram_b_rdata, font_a_rdata, font_b_rdata;

  strata_soc u_soc (.*);

  st_sram_dp #(.WORDS(1024), .INIT_FILE(ROM_INIT)) u_rom (
    .clk,
    .a_en(rom_a_en), .a_we(4'b0), .a_addr(rom_a_addr), .a_wdata(32'b0), .a_rdata(rom_a_rdata),
    .b_en(rom_b_en), .b_we(4'b0), .b_addr(rom_b_addr), .b_wdata(32'b0), .b_rdata(rom_b_rdata)
  );

  st_sram_dp #(.WORDS(16384), .INIT_FILE(RAM_INIT)) u_ram (
    .clk,
    .a_en(ram_a_en), .a_we(4'b0), .a_addr(ram_a_addr), .a_wdata(32'b0), .a_rdata(ram_a_rdata),
    .b_en(ram_b_en), .b_we(ram_b_we), .b_addr(ram_b_addr), .b_wdata(ram_b_wdata), .b_rdata(ram_b_rdata)
  );

  st_sram_dp #(.WORDS(2048)) u_vram (
    .clk,
    .a_en(vram_a_en), .a_we(vram_a_we), .a_addr(vram_a_addr), .a_wdata(vram_a_wdata), .a_rdata(vram_a_rdata),
    .b_en(vram_b_en), .b_we(4'b0), .b_addr(vram_b_addr), .b_wdata(32'b0), .b_rdata(vram_b_rdata)
  );

  st_sram_dp #(.WORDS(1024)) u_font (
    .clk,
    .a_en(font_a_en), .a_we(font_a_we), .a_addr(font_a_addr), .a_wdata(font_a_wdata), .a_rdata(font_a_rdata),
    .b_en(font_b_en), .b_we(4'b0), .b_addr(font_b_addr), .b_wdata(32'b0), .b_rdata(font_b_rdata)
  );

endmodule
