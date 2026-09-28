// Behavioral model of a dual-port synchronous SRAM macro with byte writes.
//
// This is the reference definition of the memory abstraction used by every
// Strata memory (docs/abstractions.md). On each rising clock edge, for each
// port with en = 1: if any we bit is set, the enabled bytes are written and
// rdata is unchanged; otherwise rdata <= mem[addr]. A read returns the
// contents before any write at the same edge (from either port). With en = 0
// rdata holds its value. If both ports write the same byte at the same edge,
// port B's data is stored.
//
// FPGA synthesis maps this module to block RAM; the gate-level model leaves
// it outside the synthesized netlist and simulates it with these semantics.
module st_sram_dp #(
  parameter int    WORDS     = 1024,
  parameter        INIT_FILE = ""
) (
  input  logic                     clk,
  input  logic                     a_en,
  input  logic [3:0]               a_we,
  input  logic [$clog2(WORDS)-1:0] a_addr,
  input  logic [31:0]              a_wdata,
  output logic [31:0]              a_rdata,
  input  logic                     b_en,
  input  logic [3:0]               b_we,
  input  logic [$clog2(WORDS)-1:0] b_addr,
  input  logic [31:0]              b_wdata,
  output logic [31:0]              b_rdata
);

  logic [31:0] mem [WORDS];

  initial begin
    if (INIT_FILE != "") $readmemh(INIT_FILE, mem);
  end

  always_ff @(posedge clk) begin
    if (a_en) begin
      if (a_we == 4'b0000) a_rdata <= mem[a_addr];
      for (int i = 0; i < 4; i++) if (a_we[i]) mem[a_addr][8*i +: 8] <= a_wdata[8*i +: 8];
    end
    if (b_en) begin
      if (b_we == 4'b0000) b_rdata <= mem[b_addr];
      for (int i = 0; i < 4; i++) if (b_we[i]) mem[b_addr][8*i +: 8] <= b_wdata[8*i +: 8];
    end
  end

endmodule
