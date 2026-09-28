// Strata S1 core: integer register file.
//
// x1..x31 are individual 32-bit registers (x0 reads as zero), two
// combinational read ports and one synchronous write port. Registers are
// written explicitly per index (rather than as a memory array) so that every
// architectural register keeps a stable name, gen_x[i].q, in the netlist.
module st_regfile (
  input  logic        clk,
  input  logic [4:0]  ra1,
  output logic [31:0] rd1,
  input  logic [4:0]  ra2,
  output logic [31:0] rd2,
  input  logic        we,
  input  logic [4:0]  wa,
  input  logic [31:0] wd
);

  logic [31:0] x [32];

  assign x[0] = 32'd0;

  for (genvar i = 1; i < 32; i++) begin : gen_x
    logic [31:0] q;
    always_ff @(posedge clk) begin
      if (we && wa == 5'(i)) q <= wd;
    end
    assign x[i] = q;
  end

  assign rd1 = x[ra1];
  assign rd2 = x[ra2];

endmodule
