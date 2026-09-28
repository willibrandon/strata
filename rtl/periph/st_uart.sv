// UART: 8N1 serial port with 8-entry TX and RX FIFOs.
// Register map: docs/memory-map.md#uart-0x4000_1000
module st_uart (
  input  logic        clk,
  input  logic        rst,
  input  logic        req,
  input  logic        we,
  input  logic [9:0]  addr,
  input  logic [31:0] wdata,
  output logic [31:0] rdata,
  output logic        irq,
  input  logic        rx,
  output logic        tx
);

  localparam logic [15:0] DIV_RESET = 16'd25;
  localparam logic [15:0] DIV_MIN   = 16'd4;

  logic [15:0] div_q;
  logic        rx_ie_q, tx_ie_q, overrun_q;

  logic acc_data, acc_status, acc_ctrl, acc_div;
  assign acc_data   = req && addr == 10'h000;
  assign acc_status = req && addr == 10'h001;
  assign acc_ctrl   = req && addr == 10'h002;
  assign acc_div    = req && addr == 10'h003;

  // ------------------------------------------------------------ transmitter
  logic       txf_pop, txf_empty, txf_full;
  logic [7:0] txf_dout;

  st_fifo #(.WIDTH(8), .DEPTH(8)) u_txf (
    .clk, .rst,
    .push(acc_data && we), .din(wdata[7:0]),
    .pop(txf_pop), .dout(txf_dout), .empty(txf_empty), .full(txf_full)
  );

  logic        tx_busy_q;
  logic [9:0]  tx_shift_q;   // {stop, data[7:0], start}, LSB transmitted first
  logic [3:0]  tx_bits_q;
  logic [15:0] tx_cnt_q;

  assign txf_pop = !tx_busy_q && !txf_empty;

  always_ff @(posedge clk) begin
    if (rst) begin
      tx_busy_q <= 1'b0;
      tx        <= 1'b1;
    end else begin
      if (txf_pop) begin
        tx_busy_q  <= 1'b1;
        tx_shift_q <= {1'b1, txf_dout, 1'b0};
        tx_bits_q  <= 4'd10;
        tx_cnt_q   <= div_q - 16'd1;
        tx         <= 1'b0;
      end else if (tx_busy_q) begin
        if (tx_cnt_q == 16'd0) begin
          tx_cnt_q   <= div_q - 16'd1;
          tx_bits_q  <= tx_bits_q - 4'd1;
          tx_shift_q <= {1'b1, tx_shift_q[9:1]};
          tx         <= tx_shift_q[1];
          if (tx_bits_q == 4'd1) begin
            tx_busy_q <= 1'b0;
            tx        <= 1'b1;
          end
        end else begin
          tx_cnt_q <= tx_cnt_q - 16'd1;
        end
      end
    end
  end

  // ------------------------------------------------------------ receiver
  typedef enum logic [1:0] { RX_IDLE, RX_START, RX_DATA, RX_STOP } rx_state_e;

  logic [1:0]  rx_sync_q;
  logic        rx_s;
  rx_state_e   rx_state_q;
  logic [15:0] rx_cnt_q;
  logic [2:0]  rx_bits_q;
  logic [7:0]  rx_shift_q;
  logic        rxf_push;
  logic        rxf_empty, rxf_full;
  logic [7:0]  rxf_dout;

  assign rx_s = rx_sync_q[1];

  always_ff @(posedge clk) begin
    if (rst) begin
      rx_sync_q  <= 2'b11;
      rx_state_q <= RX_IDLE;
      rxf_push   <= 1'b0;
    end else begin
      rx_sync_q <= {rx_sync_q[0], rx};
      rxf_push  <= 1'b0;
      unique case (rx_state_q)
        RX_IDLE: if (!rx_s) begin
          rx_state_q <= RX_START;
          rx_cnt_q   <= {1'b0, div_q[15:1]} - 16'd1;
        end
        RX_START: if (rx_cnt_q == 16'd0) begin
          if (rx_s) rx_state_q <= RX_IDLE;
          else      rx_state_q <= RX_DATA;
          rx_cnt_q   <= div_q - 16'd1;
          rx_bits_q  <= 3'd0;
        end else rx_cnt_q <= rx_cnt_q - 16'd1;
        RX_DATA: if (rx_cnt_q == 16'd0) begin
          rx_shift_q <= {rx_s, rx_shift_q[7:1]};
          rx_bits_q  <= rx_bits_q + 3'd1;
          rx_cnt_q   <= div_q - 16'd1;
          if (rx_bits_q == 3'd7) rx_state_q <= RX_STOP;
        end else rx_cnt_q <= rx_cnt_q - 16'd1;
        RX_STOP: if (rx_cnt_q == 16'd0) begin
          rxf_push   <= rx_s;           // a 0 stop bit is a framing error: drop
          rx_state_q <= RX_IDLE;
        end else rx_cnt_q <= rx_cnt_q - 16'd1;
        default: rx_state_q <= RX_IDLE;
      endcase
    end
  end

  logic rxf_pop;
  assign rxf_pop = acc_data && !we;

  st_fifo #(.WIDTH(8), .DEPTH(8)) u_rxf (
    .clk, .rst,
    .push(rxf_push), .din(rx_shift_q),
    .pop(rxf_pop), .dout(rxf_dout), .empty(rxf_empty), .full(rxf_full)
  );

  // ------------------------------------------------------------ registers
  always_ff @(posedge clk) begin
    if (rst) begin
      div_q     <= DIV_RESET;
      rx_ie_q   <= 1'b0;
      tx_ie_q   <= 1'b0;
      overrun_q <= 1'b0;
      rdata     <= '0;
    end else begin
      if (rxf_push && rxf_full) overrun_q <= 1'b1;
      if (req && we) begin
        if (acc_status && wdata[3]) overrun_q <= 1'b0;
        if (acc_ctrl) begin
          rx_ie_q <= wdata[0];
          tx_ie_q <= wdata[1];
        end
        if (acc_div) div_q <= (wdata[15:0] < DIV_MIN) ? DIV_MIN : wdata[15:0];
      end
      if (req && !we) begin
        unique case (addr)
          10'h000: rdata <= rxf_empty ? 32'd0 : {23'd0, 1'b1, rxf_dout};
          10'h001: rdata <= {28'd0, overrun_q, !rxf_empty, txf_empty && !tx_busy_q, txf_full};
          10'h002: rdata <= {30'd0, tx_ie_q, rx_ie_q};
          10'h003: rdata <= {16'd0, div_q};
          default: rdata <= '0;
        endcase
      end
    end
  end

  assign irq = (rx_ie_q && !rxf_empty) || (tx_ie_q && txf_empty);

endmodule
