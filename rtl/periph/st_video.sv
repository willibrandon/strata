// VIDEO: 640x480@60 text display, 80x30 cells of 8x16 pixels, 16-entry
// palette. The pixel clock is the system clock.
// Register map and memory formats: docs/memory-map.md#video-0x4000_5000
//
// Per character cell c on a visible line, the fetch pipeline is:
//   h = 8c     read the VRAM word holding cells 2*(c/2) and 2*(c/2)+1
//   h = 8c+1   select the cell; read the font word holding its glyph row
//   h = 8c+2   load the glyph row and colors into the pixel shifter
//   h = 8c+3   first pixel of the cell is at the shifter output
// so pixel x appears on the pins during h = x+4. The sync and enable signals
// pass through a matching four-stage delay.
module st_video (
  input  logic        clk,
  input  logic        rst,
  input  logic        req,
  input  logic        we,
  input  logic [9:0]  addr,
  input  logic [31:0] wdata,
  output logic [31:0] rdata,
  output logic        irq,

  // VRAM and font RAM read ports (synchronous, one-cycle latency).
  output logic        vram_en,
  output logic [10:0] vram_addr,
  input  logic [31:0] vram_rdata,
  output logic        font_en,
  output logic [9:0]  font_addr,
  input  logic [31:0] font_rdata,

  output logic [3:0]  vid_r,
  output logic [3:0]  vid_g,
  output logic [3:0]  vid_b,
  output logic        vid_hs,
  output logic        vid_vs,
  output logic        vid_de
);

  localparam int H_VISIBLE = 640, H_SYNC_START = 656, H_SYNC_END = 752, H_TOTAL = 800;
  localparam int V_VISIBLE = 480, V_SYNC_START = 490, V_SYNC_END = 492, V_TOTAL = 525;

  // ------------------------------------------------------------ registers
  logic        enable_q, vblank_ie_q, vblank_q;
  logic [31:0] frame_q;
  logic [11:0] palette_q [16];

  // ------------------------------------------------------------ timing
  logic [9:0] h_q, v_q;
  logic       h_last, v_last;
  assign h_last = (h_q == 10'(H_TOTAL - 1));
  assign v_last = (v_q == 10'(V_TOTAL - 1));

  always_ff @(posedge clk) begin
    if (rst) begin
      h_q <= '0;
      v_q <= '0;
    end else begin
      h_q <= h_last ? 10'd0 : h_q + 10'd1;
      if (h_last) v_q <= v_last ? 10'd0 : v_q + 10'd1;
    end
  end

  logic visible, hs, vs, vblank_start;
  assign visible      = (h_q < 10'(H_VISIBLE)) && (v_q < 10'(V_VISIBLE));
  assign hs           = !((h_q >= 10'(H_SYNC_START)) && (h_q < 10'(H_SYNC_END)));
  assign vs           = !((v_q >= 10'(V_SYNC_START)) && (v_q < 10'(V_SYNC_END)));
  assign vblank_start = (h_q == 10'd0) && (v_q == 10'(V_VISIBLE));

  // ------------------------------------------------------------ fetch pipeline
  logic       s1_valid_q, s1_odd_q;
  logic [3:0] s1_line_q;
  logic       s2_valid_q;
  logic [7:0] s2_attr_q;
  logic [1:0] s2_lsel_q;

  logic [15:0] s1_cell;
  assign s1_cell = s1_odd_q ? vram_rdata[31:16] : vram_rdata[15:0];

  assign vram_en   = visible && (h_q[2:0] == 3'd0);
  assign vram_addr = {v_q[8:4], h_q[9:4]};                 // row, column/2
  assign font_en   = s1_valid_q;
  assign font_addr = {s1_cell[7:0], s1_line_q[3:2]};       // glyph, row/4

  logic [7:0] glyph_row;
  assign glyph_row = font_rdata[8*s2_lsel_q +: 8];

  logic [7:0] pix_q;
  logic [3:0] fg_q, bg_q;
  logic       de_d1, de_d2, de_d3, hs_d1, hs_d2, hs_d3, vs_d1, vs_d2, vs_d3;

  always_ff @(posedge clk) begin
    if (rst) begin
      s1_valid_q <= 1'b0;
      s2_valid_q <= 1'b0;
      pix_q      <= '0;
      de_d1 <= 1'b0; de_d2 <= 1'b0; de_d3 <= 1'b0;
      hs_d1 <= 1'b1; hs_d2 <= 1'b1; hs_d3 <= 1'b1;
      vs_d1 <= 1'b1; vs_d2 <= 1'b1; vs_d3 <= 1'b1;
    end else begin
      s1_valid_q <= vram_en;
      s1_odd_q   <= h_q[3];
      s1_line_q  <= v_q[3:0];

      s2_valid_q <= s1_valid_q;
      s2_attr_q  <= s1_cell[15:8];
      s2_lsel_q  <= s1_line_q[1:0];

      if (s2_valid_q) begin
        pix_q <= glyph_row;
        fg_q  <= s2_attr_q[3:0];
        bg_q  <= s2_attr_q[7:4];
      end else begin
        pix_q <= {pix_q[6:0], 1'b0};
      end

      de_d1 <= visible; de_d2 <= de_d1; de_d3 <= de_d2;
      hs_d1 <= hs;      hs_d2 <= hs_d1; hs_d3 <= hs_d2;
      vs_d1 <= vs;      vs_d2 <= vs_d1; vs_d3 <= vs_d2;
    end
  end

  // ------------------------------------------------------------ output
  logic [11:0] color;
  assign color = palette_q[pix_q[7] ? fg_q : bg_q];

  always_ff @(posedge clk) begin
    if (rst) begin
      {vid_r, vid_g, vid_b} <= '0;
      vid_de <= 1'b0;
      vid_hs <= 1'b1;
      vid_vs <= 1'b1;
    end else begin
      {vid_r, vid_g, vid_b} <= (de_d3 && enable_q) ? color : 12'h000;
      vid_de <= de_d3;
      vid_hs <= hs_d3;
      vid_vs <= vs_d3;
    end
  end

  // ------------------------------------------------------------ control registers
  localparam logic [11:0] PALETTE_RESET [16] = '{
    12'h000, 12'h00A, 12'h0A0, 12'h0AA, 12'hA00, 12'hA0A, 12'hA50, 12'hAAA,
    12'h555, 12'h55F, 12'h5F5, 12'h5FF, 12'hF55, 12'hF5F, 12'hFF5, 12'hFFF
  };

  always_ff @(posedge clk) begin
    if (rst) begin
      enable_q    <= 1'b0;
      vblank_ie_q <= 1'b0;
      vblank_q    <= 1'b0;
      frame_q     <= '0;
      rdata       <= '0;
      for (int i = 0; i < 16; i++) palette_q[i] <= PALETTE_RESET[i];
    end else begin
      if (vblank_start) begin
        vblank_q <= 1'b1;
        frame_q  <= frame_q + 32'd1;
      end
      if (req && we) begin
        if (addr == 10'h000) begin
          enable_q    <= wdata[0];
          vblank_ie_q <= wdata[1];
        end
        if (addr == 10'h001 && wdata[1] && !vblank_start) vblank_q <= 1'b0;
        if (addr[9:4] == 6'h01) palette_q[addr[3:0]] <= wdata[11:0];
      end
      if (req && !we) begin
        if (addr == 10'h000)      rdata <= {30'd0, vblank_ie_q, enable_q};
        else if (addr == 10'h001) rdata <= {30'd0, vblank_q, v_q >= 10'(V_VISIBLE)};
        else if (addr == 10'h002) rdata <= frame_q;
        else if (addr[9:4] == 6'h01) rdata <= {20'd0, palette_q[addr[3:0]]};
        else                      rdata <= '0;
      end
    end
  end

  assign irq = vblank_ie_q && vblank_q;

endmodule
