// INTC: machine external interrupt pending/enable.
// Register map: docs/memory-map.md#intc-0x4000_6000
module st_intc #(
  parameter int N = 4
) (
  input  logic         clk,
  input  logic         rst,
  input  logic         req,
  input  logic         we,
  input  logic [9:0]   addr,
  input  logic [31:0]  wdata,
  output logic [31:0]  rdata,
  input  logic [N-1:0] sources,
  output logic         meip
);

  logic [N-1:0] enable_q;

  always_ff @(posedge clk) begin
    if (rst) begin
      enable_q <= '0;
      rdata    <= '0;
    end else begin
      if (req && we && addr == 10'h001) enable_q <= wdata[N-1:0];
      if (req && !we) begin
        unique case (addr)
          10'h000: rdata <= {{(32-N){1'b0}}, sources};
          10'h001: rdata <= {{(32-N){1'b0}}, enable_q};
          default: rdata <= '0;
        endcase
      end
    end
  end

  assign meip = |(sources & enable_q);

endmodule
