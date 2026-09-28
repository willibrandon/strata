// GPIO: eight LEDs and seven buttons with sticky press events.
// Register map: docs/memory-map.md#gpio-0x4000_3000
module st_gpio (
  input  logic        clk,
  input  logic        rst,
  input  logic        req,
  input  logic        we,
  input  logic [9:0]  addr,
  input  logic [31:0] wdata,
  output logic [31:0] rdata,
  output logic        irq,
  input  logic [6:0]  btn,
  output logic [7:0]  led
);

  logic [6:0] sync0_q, sync1_q, prev_q, edges_q, irq_en_q;

  always_ff @(posedge clk) begin
    if (rst) begin
      sync0_q  <= '0;
      sync1_q  <= '0;
      prev_q   <= '0;
      edges_q  <= '0;
      irq_en_q <= '0;
      led      <= '0;
      rdata    <= '0;
    end else begin
      sync0_q <= btn;
      sync1_q <= sync0_q;
      prev_q  <= sync1_q;

      // New presses set EDGES; a W1C write clears the written bits (a press
      // in the same cycle wins).
      if (req && we && addr == 10'h002) edges_q <= (edges_q & ~wdata[6:0]) | (sync1_q & ~prev_q);
      else                              edges_q <= edges_q | (sync1_q & ~prev_q);

      if (req && we && addr == 10'h000) led      <= wdata[7:0];
      if (req && we && addr == 10'h003) irq_en_q <= wdata[6:0];

      if (req && !we) begin
        unique case (addr)
          10'h000: rdata <= {24'd0, led};
          10'h001: rdata <= {25'd0, sync1_q};
          10'h002: rdata <= {25'd0, edges_q};
          10'h003: rdata <= {25'd0, irq_en_q};
          default: rdata <= '0;
        endcase
      end
    end
  end

  assign irq = |(edges_q & irq_en_q);

endmodule
