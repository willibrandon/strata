// Data-bus decoder: routes loads and stores to the ROM, RAM, VRAM and font
// memory ports and to the MMIO device pages. Unmapped addresses, stores to
// ROM, and non-word MMIO accesses get an error response and touch nothing.
// Protocol: docs/bus.md. Address map: docs/memory-map.md.
module st_dbus #(
  parameter int NDEV = 7
) (
  input  logic        clk,
  input  logic        rst,

  input  logic        cmd_valid,
  input  logic [31:0] cmd_addr,
  input  logic        cmd_we,
  input  logic [3:0]  cmd_be,
  input  logic [31:0] cmd_wdata,
  output logic        cmd_ready,
  output logic        rsp_valid,
  output logic [31:0] rsp_rdata,
  output logic        rsp_err,

  output logic        rom_en,
  output logic [9:0]  rom_addr,
  input  logic [31:0] rom_rdata,

  output logic        ram_en,
  output logic [3:0]  ram_we,
  output logic [13:0] ram_addr,
  output logic [31:0] ram_wdata,
  input  logic [31:0] ram_rdata,

  output logic        vram_en,
  output logic [3:0]  vram_we,
  output logic [10:0] vram_addr,
  output logic [31:0] vram_wdata,
  input  logic [31:0] vram_rdata,

  output logic        font_en,
  output logic [3:0]  font_we,
  output logic [9:0]  font_addr,
  output logic [31:0] font_wdata,
  input  logic [31:0] font_rdata,

  // MMIO: one request strobe per 4 KiB device page.
  output logic [NDEV-1:0]    dev_req,
  output logic               dev_we,
  output logic [9:0]         dev_addr,
  output logic [31:0]        dev_wdata,
  input  logic [NDEV*32-1:0] dev_rdata
);

  logic sel_rom, sel_ram, sel_vram, sel_font, sel_mmio, err;
  logic [2:0] page;

  assign page     = cmd_addr[14:12];
  assign sel_rom  = cmd_addr[31:12] == 20'h0_0000 && !cmd_we;
  assign sel_ram  = cmd_addr[31:16] == 16'h1000;
  assign sel_vram = cmd_addr[31:13] == 19'h1_0000;
  assign sel_font = cmd_addr[31:12] == 20'h2_0010;
  assign sel_mmio = cmd_addr[31:15] == 17'h0_8000 && 32'(page) < NDEV && cmd_be == 4'b1111;
  assign err      = !(sel_rom || sel_ram || sel_vram || sel_font || sel_mmio);

  logic [3:0] we;
  assign we = cmd_we ? cmd_be : 4'b0000;

  assign cmd_ready = 1'b1;

  assign rom_en     = cmd_valid && sel_rom;
  assign rom_addr   = cmd_addr[11:2];
  assign ram_en     = cmd_valid && sel_ram;
  assign ram_we     = we;
  assign ram_addr   = cmd_addr[15:2];
  assign ram_wdata  = cmd_wdata;
  assign vram_en    = cmd_valid && sel_vram;
  assign vram_we    = we;
  assign vram_addr  = cmd_addr[12:2];
  assign vram_wdata = cmd_wdata;
  assign font_en    = cmd_valid && sel_font;
  assign font_we    = we;
  assign font_addr  = cmd_addr[11:2];
  assign font_wdata = cmd_wdata;

  for (genvar i = 0; i < NDEV; i++) begin : gen_req
    assign dev_req[i] = cmd_valid && sel_mmio && page == 3'(i);
  end
  assign dev_we    = cmd_we;
  assign dev_addr  = cmd_addr[11:2];
  assign dev_wdata = cmd_wdata;

  // Response: exactly one cycle after each command.
  typedef enum logic [2:0] { SRC_ERR, SRC_ROM, SRC_RAM, SRC_VRAM, SRC_FONT, SRC_MMIO } src_e;
  src_e       src_q;
  logic [2:0] page_q;

  always_ff @(posedge clk) begin
    if (rst) begin
      rsp_valid <= 1'b0;
    end else begin
      rsp_valid <= cmd_valid;
      page_q    <= page;
      if (sel_rom)       src_q <= SRC_ROM;
      else if (sel_ram)  src_q <= SRC_RAM;
      else if (sel_vram) src_q <= SRC_VRAM;
      else if (sel_font) src_q <= SRC_FONT;
      else if (sel_mmio) src_q <= SRC_MMIO;
      else               src_q <= SRC_ERR;
    end
  end

  always_comb begin
    unique case (src_q)
      SRC_ROM:  rsp_rdata = rom_rdata;
      SRC_RAM:  rsp_rdata = ram_rdata;
      SRC_VRAM: rsp_rdata = vram_rdata;
      SRC_FONT: rsp_rdata = font_rdata;
      SRC_MMIO: rsp_rdata = dev_rdata[32*page_q +: 32];
      default:  rsp_rdata = 32'd0;
    endcase
  end
  assign rsp_err = rsp_valid && (src_q == SRC_ERR);

endmodule
