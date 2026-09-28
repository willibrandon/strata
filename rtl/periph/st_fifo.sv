// Small synchronous FIFO built from flip-flops. Push is ignored when full,
// pop when empty. The head entry is available combinationally.
module st_fifo #(
  parameter int WIDTH = 8,
  parameter int DEPTH = 8          // power of two
) (
  input  logic             clk,
  input  logic             rst,
  input  logic             push,
  input  logic [WIDTH-1:0] din,
  input  logic             pop,
  output logic [WIDTH-1:0] dout,
  output logic             empty,
  output logic             full
);

  localparam int AW = $clog2(DEPTH);

  logic [WIDTH-1:0] mem [DEPTH];
  logic [AW-1:0]    rd_q, wr_q;
  logic [AW:0]      count_q;

  assign empty = (count_q == '0);
  assign full  = (count_q == (AW+1)'(DEPTH));
  assign dout  = mem[rd_q];

  logic do_push, do_pop;
  assign do_push = push && !full;
  assign do_pop  = pop && !empty;

  always_ff @(posedge clk) begin
    if (rst) begin
      rd_q    <= '0;
      wr_q    <= '0;
      count_q <= '0;
    end else begin
      if (do_push) wr_q <= wr_q + 1'b1;
      if (do_pop)  rd_q <= rd_q + 1'b1;
      unique case ({do_push, do_pop})
        2'b10:   count_q <= count_q + 1'b1;
        2'b01:   count_q <= count_q - 1'b1;
        default: ;
      endcase
    end
  end

  always_ff @(posedge clk) begin
    if (do_push) mem[wr_q] <= din;
  end

endmodule
