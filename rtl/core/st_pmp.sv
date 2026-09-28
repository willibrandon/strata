// Strata S1 core: physical memory protection checker (combinational).
//
// Four entries, granularity 4 bytes. Implements the matching and priority
// rules of docs/isa.md for one access. Accesses are naturally aligned and at
// most 4 bytes, so matching the word address is exact.
module st_pmp
  import st_pkg::*;
#(
  parameter int ENTRIES = 4
) (
  input  logic [ENTRIES*8-1:0]  cfg,      // pmpcfg bytes, entry i at [8i+7:8i]
  input  logic [ENTRIES*32-1:0] addr,     // pmpaddr registers, entry i at [32i+31:32i]
  input  logic [1:0]            mode,     // effective privilege of the access
  input  logic [31:0]           paddr,    // byte address of the access
  input  logic                  acc_r,
  input  logic                  acc_w,
  input  logic                  acc_x,
  output logic                  ok
);

  // Word address, i.e. paddr[33:2] with paddr[33:32] = 0.
  logic [31:0] wa;
  assign wa = {2'b00, paddr[31:2]};

  logic [ENTRIES-1:0] match;
  logic [ENTRIES-1:0] perm;

  for (genvar i = 0; i < ENTRIES; i++) begin : gen_entry
    logic [7:0]  c;
    logic [31:0] top;
    logic [31:0] bottom;
    logic [31:0] care;   // NAPOT: bits of the address that must equal pmpaddr

    assign c   = cfg[8*i +: 8];
    assign top = addr[32*i +: 32];
    if (i == 0) begin : g_base0
      assign bottom = 32'd0;
    end else begin : g_base
      assign bottom = addr[32*(i-1) +: 32];
    end
    assign care = ~(top ^ (top + 32'd1));

    always_comb begin
      unique case (c[4:3])
        2'd1:    match[i] = (wa >= bottom) && (wa < top);        // TOR
        2'd2:    match[i] = (wa == top);                         // NA4
        2'd3:    match[i] = ((wa ^ top) & care) == 32'd0;        // NAPOT
        default: match[i] = 1'b0;                                // OFF
      endcase
    end

    assign perm[i] = (!acc_r || c[0]) && (!acc_w || c[1]) && (!acc_x || c[2]);
  end

  // Lowest-numbered matching entry decides.
  always_comb begin
    ok = (mode == PRV_M);
    for (int i = ENTRIES - 1; i >= 0; i--) begin
      if (match[i]) begin
        ok = (mode == PRV_M && !cfg[8*i+7]) ? 1'b1 : perm[i];
      end
    end
  end

endmodule
