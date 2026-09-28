// TMDS 8b/10b encoder (DVI 1.0, section 3.2): transition minimization, then
// DC balancing with a running disparity; control tokens during blanking.
module st_tmds_encoder (
  input  logic       clk,
  input  logic       de,
  input  logic [7:0] d,
  input  logic [1:0] c,
  output logic [9:0] q
);

  function automatic logic [3:0] ones8(input logic [7:0] v);
    ones8 = 4'd0;
    for (int i = 0; i < 8; i++) ones8 = ones8 + 4'(v[i]);
  endfunction

  // Stage 1: transition-minimized word q_m.
  logic [3:0] n1_d;
  logic       use_xnor;
  logic [8:0] q_m;
  assign n1_d     = ones8(d);
  assign use_xnor = (n1_d > 4'd4) || (n1_d == 4'd4 && d[0] == 1'b0);
  always_comb begin
    q_m[0] = d[0];
    for (int i = 1; i < 8; i++) q_m[i] = use_xnor ? !(q_m[i-1] ^ d[i]) : (q_m[i-1] ^ d[i]);
    q_m[8] = !use_xnor;
  end

  // Stage 2: DC balance. cnt is the running disparity (ones minus zeros).
  logic signed [4:0] cnt_q;
  logic [3:0]        n1_q, n0_q;
  logic signed [4:0] diff;   // n1 - n0 of q_m[7:0]
  assign n1_q = ones8(q_m[7:0]);
  assign n0_q = 4'd8 - n1_q;
  assign diff = $signed({1'b0, n1_q}) - $signed({1'b0, n0_q});

  always_ff @(posedge clk) begin
    if (!de) begin
      cnt_q <= '0;
      unique case (c)
        2'b00: q <= 10'b1101010100;
        2'b01: q <= 10'b0010101011;
        2'b10: q <= 10'b0101010100;
        default: q <= 10'b1010101011;
      endcase
    end else if (cnt_q == 0 || n1_q == n0_q) begin
      q <= {!q_m[8], q_m[8], q_m[8] ? q_m[7:0] : ~q_m[7:0]};
      cnt_q <= q_m[8] ? cnt_q + diff : cnt_q - diff;
    end else if ((cnt_q > 0 && n1_q > n0_q) || (cnt_q < 0 && n0_q > n1_q)) begin
      q <= {1'b1, q_m[8], ~q_m[7:0]};
      cnt_q <= cnt_q + (q_m[8] ? 5'sd2 : 5'sd0) - diff;
    end else begin
      q <= {1'b0, q_m[8], q_m[7:0]};
      cnt_q <= cnt_q - (q_m[8] ? 5'sd0 : 5'sd2) + diff;
    end
  end

endmodule
