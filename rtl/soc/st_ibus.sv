// Instruction-bus decoder: routes fetches to the boot ROM or RAM read ports
// and answers every other address with an error response.
// Protocol: docs/bus.md. Address map: docs/memory-map.md.
module st_ibus (
  input  logic        clk,
  input  logic        rst,

  input  logic        cmd_valid,
  input  logic [31:0] cmd_addr,
  output logic        cmd_ready,
  output logic        rsp_valid,
  output logic [31:0] rsp_rdata,
  output logic        rsp_err,

  output logic        rom_en,
  output logic [9:0]  rom_addr,
  input  logic [31:0] rom_rdata,
  output logic        ram_en,
  output logic [13:0] ram_addr,
  input  logic [31:0] ram_rdata
);

  logic sel_rom, sel_ram;
  assign sel_rom = cmd_addr[31:12] == 20'h0_0000;
  assign sel_ram = cmd_addr[31:16] == 16'h1000;

  assign cmd_ready = 1'b1;
  assign rom_en    = cmd_valid && sel_rom;
  assign rom_addr  = cmd_addr[11:2];
  assign ram_en    = cmd_valid && sel_ram;
  assign ram_addr  = cmd_addr[15:2];

  logic src_rom_q, src_ram_q;
  always_ff @(posedge clk) begin
    if (rst) begin
      rsp_valid <= 1'b0;
    end else begin
      rsp_valid <= cmd_valid;
      src_rom_q <= sel_rom;
      src_ram_q <= sel_ram;
    end
  end

  assign rsp_rdata = src_ram_q ? ram_rdata : src_rom_q ? rom_rdata : 32'd0;
  assign rsp_err   = rsp_valid && !src_rom_q && !src_ram_q;

endmodule
