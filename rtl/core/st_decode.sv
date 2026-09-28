// Strata S1 core: instruction decoder (combinational).
//
// Recognizes exactly the encodings listed in docs/isa.md. Anything else sets
// ctrl.illegal and clears every side-effect flag. CSR-address legality depends
// on the privilege mode and is checked when the instruction executes in W.
module st_decode
  import st_pkg::*;
(
  input  logic [31:0] insn,
  output ctrl_t       ctrl,
  output logic [31:0] imm,
  output logic [4:0]  rs1,
  output logic [4:0]  rs2,
  output logic [4:0]  rd
);

  logic [6:0] opcode;
  logic [2:0] funct3;
  logic [6:0] funct7;

  assign opcode = insn[6:0];
  assign funct3 = insn[14:12];
  assign funct7 = insn[31:25];
  assign rd     = insn[11:7];
  assign rs1    = insn[19:15];
  assign rs2    = insn[24:20];

  logic [31:0] imm_i, imm_s, imm_b, imm_u, imm_j;
  assign imm_i = {{20{insn[31]}}, insn[31:20]};
  assign imm_s = {{20{insn[31]}}, insn[31:25], insn[11:7]};
  assign imm_b = {{19{insn[31]}}, insn[31], insn[7], insn[30:25], insn[11:8], 1'b0};
  assign imm_u = {insn[31:12], 12'b0};
  assign imm_j = {{11{insn[31]}}, insn[31], insn[19:12], insn[20], insn[30:21], 1'b0};

  ctrl_t c;
  logic  legal;
  logic  rd_written;  // the instruction architecturally writes rd (if rd != 0)

  always_comb begin
    c          = '0;
    c.alu_op   = ALU_ADD;
    c.a_sel    = A_RS1;
    c.mem_size = mem_size_e'(funct3[1:0]);
    c.mem_unsigned = funct3[2];
    c.csr_op   = CSR_NONE;
    imm        = imm_i;
    legal      = 1'b0;
    rd_written = 1'b0;

    unique case (opcode)
      OPC_LUI: begin
        legal = 1'b1; rd_written = 1'b1;
        c.a_sel = A_ZERO; c.b_imm = 1'b1; imm = imm_u;
      end
      OPC_AUIPC: begin
        legal = 1'b1; rd_written = 1'b1;
        c.a_sel = A_PC; c.b_imm = 1'b1; imm = imm_u;
      end
      OPC_JAL: begin
        legal = 1'b1; rd_written = 1'b1;
        c.is_jal = 1'b1; imm = imm_j;
      end
      OPC_JALR: begin
        legal = (funct3 == 3'b000); rd_written = 1'b1;
        c.is_jalr = 1'b1; c.uses_rs1 = 1'b1; imm = imm_i;
      end
      OPC_BRANCH: begin
        legal = (funct3 != 3'b010) && (funct3 != 3'b011);
        c.is_branch = 1'b1; c.uses_rs1 = 1'b1; c.uses_rs2 = 1'b1; imm = imm_b;
      end
      OPC_LOAD: begin
        legal = (funct3 == 3'b000) || (funct3 == 3'b001) || (funct3 == 3'b010) ||
                (funct3 == 3'b100) || (funct3 == 3'b101);
        rd_written = 1'b1;
        c.is_load = 1'b1; c.uses_rs1 = 1'b1; c.b_imm = 1'b1; imm = imm_i;
      end
      OPC_STORE: begin
        legal = (funct3 == 3'b000) || (funct3 == 3'b001) || (funct3 == 3'b010);
        c.is_store = 1'b1; c.uses_rs1 = 1'b1; c.uses_rs2 = 1'b1; c.b_imm = 1'b1; imm = imm_s;
      end
      OPC_OP_IMM: begin
        rd_written = 1'b1; c.uses_rs1 = 1'b1; c.b_imm = 1'b1; imm = imm_i;
        unique case (funct3)
          3'b000: begin legal = 1'b1; c.alu_op = ALU_ADD;  end
          3'b010: begin legal = 1'b1; c.alu_op = ALU_SLT;  end
          3'b011: begin legal = 1'b1; c.alu_op = ALU_SLTU; end
          3'b100: begin legal = 1'b1; c.alu_op = ALU_XOR;  end
          3'b110: begin legal = 1'b1; c.alu_op = ALU_OR;   end
          3'b111: begin legal = 1'b1; c.alu_op = ALU_AND;  end
          3'b001: begin legal = (funct7 == 7'b0000000); c.alu_op = ALU_SLL; end
          3'b101: begin
            legal = (funct7 == 7'b0000000) || (funct7 == 7'b0100000);
            if (funct7[5]) c.alu_op = ALU_SRA;
            else           c.alu_op = ALU_SRL;
          end
          default: ;
        endcase
      end
      OPC_OP: begin
        rd_written = 1'b1; c.uses_rs1 = 1'b1; c.uses_rs2 = 1'b1;
        if (funct7 == 7'b0000000) begin
          legal = 1'b1;
          unique case (funct3)
            3'b000: c.alu_op = ALU_ADD;
            3'b001: c.alu_op = ALU_SLL;
            3'b010: c.alu_op = ALU_SLT;
            3'b011: c.alu_op = ALU_SLTU;
            3'b100: c.alu_op = ALU_XOR;
            3'b101: c.alu_op = ALU_SRL;
            3'b110: c.alu_op = ALU_OR;
            3'b111: c.alu_op = ALU_AND;
            default: ;
          endcase
        end else if (funct7 == 7'b0100000) begin
          legal = (funct3 == 3'b000) || (funct3 == 3'b101);
          if (funct3 == 3'b000) c.alu_op = ALU_SUB;
          else                  c.alu_op = ALU_SRA;
        end
      end
      OPC_MISC_MEM: begin
        // FENCE is a no-op on this in-order, single-access pipeline.
        legal = (funct3 == 3'b000) || (funct3 == 3'b001);
        c.is_fence_i = (funct3 == 3'b001);
      end
      OPC_SYSTEM: begin
        if (funct3 == 3'b000) begin
          unique case (insn)
            32'h0000_0073: begin legal = 1'b1; c.is_ecall  = 1'b1; end
            32'h0010_0073: begin legal = 1'b1; c.is_ebreak = 1'b1; end
            32'h3020_0073: begin legal = 1'b1; c.is_mret   = 1'b1; end
            32'h1050_0073: begin legal = 1'b1; c.is_wfi    = 1'b1; end
            default: ;
          endcase
        end else if (funct3 != 3'b100) begin
          legal      = 1'b1;
          rd_written = 1'b1;
          c.csr_op   = csr_op_e'(funct3[1:0]);
          c.csr_imm  = funct3[2];
          c.uses_rs1 = !funct3[2];
          // CSRRW/CSRRWI always write; the set/clear forms write iff the
          // rs1 field (register index or immediate) is nonzero.
          c.csr_write = (funct3[1:0] == 2'b01) || (rs1 != 5'd0);
        end
      end
      default: ;
    endcase

    if (insn[1:0] != 2'b11) legal = 1'b0;

    c.writes_rd = rd_written && (rd != 5'd0);

    if (!legal) begin
      c = '0;
      c.illegal = 1'b1;
    end
  end

  assign ctrl = c;

endmodule
