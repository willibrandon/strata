// Strata S1 core: integer ALU (combinational).
module st_alu
  import st_pkg::*;
(
  input  alu_op_e     op,
  input  logic [31:0] a,
  input  logic [31:0] b,
  output logic [31:0] y
);

  logic [32:0] diff;     // a - b with borrow, shared by SUB/SLT/SLTU
  logic        lt_signed;
  logic [4:0]  shamt;

  assign diff      = {1'b0, a} - {1'b0, b};
  assign lt_signed = (a[31] ^ b[31]) ? a[31] : diff[32];
  assign shamt     = b[4:0];

  always_comb begin
    unique case (op)
      ALU_ADD:  y = a + b;
      ALU_SUB:  y = diff[31:0];
      ALU_SLL:  y = a << shamt;
      ALU_SLT:  y = {31'b0, lt_signed};
      ALU_SLTU: y = {31'b0, diff[32]};
      ALU_XOR:  y = a ^ b;
      ALU_SRL:  y = a >> shamt;
      ALU_SRA:  y = $unsigned($signed(a) >>> shamt);
      ALU_OR:   y = a | b;
      ALU_AND:  y = a & b;
      default:  y = a + b;
    endcase
  end

endmodule
