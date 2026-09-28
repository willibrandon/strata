// DVI output over the ULX3S HDMI connector: three TMDS channels plus the
// clock channel, serialized 10:1 with DDR output registers at 5x the pixel
// clock. The pixel and bit clocks come from one PLL and are phase locked.
module st_hdmi_out (
  input  logic       clk_px,      // 25 MHz pixel clock
  input  logic       clk_bit,     // 125 MHz, 5x clk_px, phase aligned
  input  logic [3:0] r,
  input  logic [3:0] g,
  input  logic [3:0] b,
  input  logic       hs,
  input  logic       vs,
  input  logic       de,
  output logic [3:0] tmds         // [0] blue, [1] green, [2] red, [3] clock
);

  logic [9:0] q_b, q_g, q_r;

  // 4-bit color components are expanded to 8 bits by replication.
  st_tmds_encoder u_enc_b (.clk(clk_px), .de, .d({b, b}), .c({vs, hs}), .q(q_b));
  st_tmds_encoder u_enc_g (.clk(clk_px), .de, .d({g, g}), .c(2'b00),    .q(q_g));
  st_tmds_encoder u_enc_r (.clk(clk_px), .de, .d({r, r}), .c(2'b00),    .q(q_r));

  // Find the pixel-clock edge in the bit-clock domain once: toggle a bit
  // every pixel clock and watch it through a synchronizer. The first change
  // seen after configuration fixes the phase of a free-running divide-by-5
  // counter; the clocks come from one locked PLL, so the alignment holds and
  // is never revisited (re-aligning on every edge could slip by one bit if
  // the synchronizer resolved differently later).
  //
  // Bit-clock edges are numbered from the pixel edge that flips toggle_px
  // (edge 0). tsync[0] takes the new value at edge 0 or 1 (depending on
  // whether the coincident edge sees it), the counter restarts two edges
  // later, and the load happens when it has counted to 3: at edge 6 or 7,
  // which is one or two bit clocks after the next pixel edge (edge 5). The
  // load never coincides with a pixel edge, so q_* has settled.
  logic toggle_px = 1'b0;
  always_ff @(posedge clk_px) toggle_px <= !toggle_px;

  logic [2:0] tsync = '0;
  logic [2:0] phase = '0;
  logic       aligned = 1'b0;
  logic       load;
  always_ff @(posedge clk_bit) begin
    tsync <= {tsync[1:0], toggle_px};
    if (!aligned && tsync[2] != tsync[1]) begin
      aligned <= 1'b1;
      phase <= 3'd0;
    end else begin
      phase <= (phase == 3'd4) ? 3'd0 : phase + 3'd1;
    end
  end
  assign load = aligned && (phase == 3'd3);

  logic [9:0] sh_b, sh_g, sh_r, sh_c;
  always_ff @(posedge clk_bit) begin
    if (load) begin
      sh_b <= q_b;
      sh_g <= q_g;
      sh_r <= q_r;
      sh_c <= 10'b0000011111;
    end else begin
      sh_b <= {2'b00, sh_b[9:2]};
      sh_g <= {2'b00, sh_g[9:2]};
      sh_r <= {2'b00, sh_r[9:2]};
      sh_c <= {2'b00, sh_c[9:2]};
    end
  end

  // Least significant bit first: D0 goes out on the rising edge.
  ODDRX1F u_ddr_b (.SCLK(clk_bit), .RST(1'b0), .D0(sh_b[0]), .D1(sh_b[1]), .Q(tmds[0]));
  ODDRX1F u_ddr_g (.SCLK(clk_bit), .RST(1'b0), .D0(sh_g[0]), .D1(sh_g[1]), .Q(tmds[1]));
  ODDRX1F u_ddr_r (.SCLK(clk_bit), .RST(1'b0), .D0(sh_r[0]), .D1(sh_r[1]), .Q(tmds[2]));
  ODDRX1F u_ddr_c (.SCLK(clk_bit), .RST(1'b0), .D0(sh_c[0]), .D1(sh_c[1]), .Q(tmds[3]));

endmodule
