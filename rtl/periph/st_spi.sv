// SPI: byte-oriented SPI master, mode 0, MSB first (boot flash interface).
// Register map: docs/memory-map.md#spi-0x4000_4000
module st_spi (
  input  logic        clk,
  input  logic        rst,
  input  logic        req,
  input  logic        we,
  input  logic [9:0]  addr,
  input  logic [31:0] wdata,
  output logic [31:0] rdata,
  output logic        irq,
  output logic        cs_n,
  output logic        sck,
  output logic        mosi,
  input  logic        miso
);

  logic       cs_q, done_ie_q, done_q, busy_q;
  logic [7:0] half_q, cnt_q, shift_q, rx_q;
  logic [2:0] nbits_q;   // bits completed in the current transfer

  assign cs_n = !cs_q;
  assign mosi = shift_q[7];

  always_ff @(posedge clk) begin
    if (rst) begin
      cs_q      <= 1'b0;
      done_ie_q <= 1'b0;
      done_q    <= 1'b0;
      busy_q    <= 1'b0;
      half_q    <= '0;
      shift_q   <= '0;
      rx_q      <= '0;
      sck       <= 1'b0;
      rdata     <= '0;
    end else begin
      if (busy_q) begin
        if (cnt_q == 8'd0) begin
          cnt_q <= half_q;
          if (!sck) begin
            sck  <= 1'b1;                      // rising edge: sample MISO
            rx_q <= {rx_q[6:0], miso};
          end else begin
            sck <= 1'b0;                       // falling edge: next MOSI bit
            if (nbits_q == 3'd7) begin
              busy_q <= 1'b0;
              done_q <= 1'b1;
            end else begin
              nbits_q <= nbits_q + 3'd1;
              shift_q <= {shift_q[6:0], 1'b0};
            end
          end
        end else begin
          cnt_q <= cnt_q - 8'd1;
        end
      end

      if (req && we) begin
        unique case (addr)
          10'h000: if (!busy_q) begin
            busy_q  <= 1'b1;
            shift_q <= wdata[7:0];
            nbits_q <= 3'd0;
            cnt_q   <= half_q;
          end
          10'h002: begin
            cs_q      <= wdata[0];
            half_q    <= wdata[15:8];
            done_ie_q <= wdata[16];
          end
          10'h003: if (wdata[0]) done_q <= 1'b0;
          default: ;
        endcase
      end

      if (req && !we) begin
        unique case (addr)
          10'h000: rdata <= {24'd0, rx_q};
          10'h001: rdata <= {31'd0, busy_q};
          10'h002: rdata <= {15'd0, done_ie_q, half_q, 7'd0, cs_q};
          10'h003: rdata <= {31'd0, done_q};
          default: rdata <= '0;
        endcase
      end
    end
  end

  assign irq = done_ie_q && done_q;

endmodule
