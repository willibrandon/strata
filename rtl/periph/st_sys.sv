// SYS: identification, scratch register, halt, and software reset.
// Register map: docs/memory-map.md#sys-0x4000_0000
module st_sys #(
  parameter logic [31:0] CLK_HZ    = 32'd25_000_000,
  parameter logic [31:0] RAM_BYTES = 32'd65536
) (
  input  logic        clk,
  input  logic        rst,
  input  logic        req,
  input  logic        we,
  input  logic [9:0]  addr,       // word offset within the page
  input  logic [31:0] wdata,
  output logic [31:0] rdata,
  output logic        halted,
  output logic [7:0]  halt_code,
  output logic        reset_req   // one-cycle request for a machine reset
);

  localparam logic [31:0] ID      = 32'h4152_5453;  // "STRA"
  localparam logic [31:0] VERSION = 32'h0001_0000;
  localparam logic [31:0] RESET_MAGIC = 32'h5245_5345;

  logic [31:0] scratch_q;

  always_ff @(posedge clk) begin
    if (rst) begin
      scratch_q <= '0;
      halted    <= 1'b0;
      halt_code <= '0;
      reset_req <= 1'b0;
      rdata     <= '0;
    end else begin
      reset_req <= 1'b0;
      if (req && we) begin
        unique case (addr)
          10'h004: scratch_q <= wdata;
          10'h008: if (!halted) begin halted <= 1'b1; halt_code <= wdata[7:0]; end
          10'h009: reset_req <= (wdata == RESET_MAGIC);
          default: ;
        endcase
      end
      if (req && !we) begin
        unique case (addr)
          10'h000: rdata <= ID;
          10'h001: rdata <= VERSION;
          10'h002: rdata <= CLK_HZ;
          10'h003: rdata <= RAM_BYTES;
          10'h004: rdata <= scratch_q;
          default: rdata <= '0;
        endcase
      end
    end
  end

endmodule
