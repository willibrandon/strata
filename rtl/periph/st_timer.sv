// TIMER: 64-bit mtime/mtimecmp and the machine software-interrupt bit
// (a re-based subset of the RISC-V CLINT).
// Register map: docs/memory-map.md#timer-0x4000_2000
module st_timer (
  input  logic        clk,
  input  logic        rst,
  input  logic        req,
  input  logic        we,
  input  logic [9:0]  addr,
  input  logic [31:0] wdata,
  output logic [31:0] rdata,
  output logic [63:0] mtime,
  output logic        mtip,
  output logic        msip
);

  logic [63:0] mtime_q, mtimecmp_q;
  logic        msip_q;

  assign mtime = mtime_q;
  assign mtip  = mtime_q >= mtimecmp_q;
  assign msip  = msip_q;

  logic wr;
  assign wr = req && we;

  always_ff @(posedge clk) begin
    if (rst) begin
      mtime_q    <= '0;
      mtimecmp_q <= '1;
      msip_q     <= 1'b0;
      rdata      <= '0;
    end else begin
      // A write replaces one half and suppresses the increment this cycle.
      if (wr && addr == 10'h000)      mtime_q[31:0]  <= wdata;
      else if (wr && addr == 10'h001) mtime_q[63:32] <= wdata;
      else                            mtime_q        <= mtime_q + 64'd1;

      if (wr && addr == 10'h002) mtimecmp_q[31:0]  <= wdata;
      if (wr && addr == 10'h003) mtimecmp_q[63:32] <= wdata;
      if (wr && addr == 10'h004) msip_q            <= wdata[0];

      if (req && !we) begin
        unique case (addr)
          10'h000: rdata <= mtime_q[31:0];
          10'h001: rdata <= mtime_q[63:32];
          10'h002: rdata <= mtimecmp_q[31:0];
          10'h003: rdata <= mtimecmp_q[63:32];
          10'h004: rdata <= {31'd0, msip_q};
          default: rdata <= '0;
        endcase
      end
    end
  end

endmodule
