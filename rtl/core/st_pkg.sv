// Strata S1 core: shared definitions.
package st_pkg;

  // Major opcodes (insn[6:0]).
  localparam logic [6:0] OPC_LUI      = 7'b0110111;
  localparam logic [6:0] OPC_AUIPC    = 7'b0010111;
  localparam logic [6:0] OPC_JAL      = 7'b1101111;
  localparam logic [6:0] OPC_JALR     = 7'b1100111;
  localparam logic [6:0] OPC_BRANCH   = 7'b1100011;
  localparam logic [6:0] OPC_LOAD     = 7'b0000011;
  localparam logic [6:0] OPC_STORE    = 7'b0100011;
  localparam logic [6:0] OPC_OP_IMM   = 7'b0010011;
  localparam logic [6:0] OPC_OP       = 7'b0110011;
  localparam logic [6:0] OPC_MISC_MEM = 7'b0001111;
  localparam logic [6:0] OPC_SYSTEM   = 7'b1110011;

  typedef enum logic [3:0] {
    ALU_ADD, ALU_SUB, ALU_SLL, ALU_SLT, ALU_SLTU,
    ALU_XOR, ALU_SRL, ALU_SRA, ALU_OR, ALU_AND
  } alu_op_e;

  // First ALU operand.
  typedef enum logic [1:0] { A_RS1, A_PC, A_ZERO } a_sel_e;

  // Memory access size (insn funct3[1:0]).
  typedef enum logic [1:0] { SZ_B = 2'd0, SZ_H = 2'd1, SZ_W = 2'd2 } mem_size_e;

  // CSR operation (insn funct3[1:0]).
  typedef enum logic [1:0] { CSR_NONE = 2'd0, CSR_RW = 2'd1, CSR_RS = 2'd2, CSR_RC = 2'd3 } csr_op_e;

  // Decoded control for one instruction. Produced in D, carried down the pipe.
  typedef struct packed {
    logic      illegal;     // not a legal encoding (CSR address legality is checked in W)
    alu_op_e   alu_op;
    a_sel_e    a_sel;
    logic      b_imm;       // second ALU operand is the immediate (else rs2)
    logic      uses_rs1;
    logic      uses_rs2;
    logic      writes_rd;   // rd != x0 and the instruction writes rd
    logic      is_branch;   // conditional branch; funct3 selects the condition
    logic      is_jal;
    logic      is_jalr;
    logic      is_load;
    logic      is_store;
    mem_size_e mem_size;
    logic      mem_unsigned;
    csr_op_e   csr_op;      // CSR_NONE for non-CSR instructions
    logic      csr_imm;     // CSR source is the 5-bit zero-extended rs1 field
    logic      csr_write;   // the CSR instruction writes its CSR
    logic      is_ecall;
    logic      is_ebreak;
    logic      is_mret;
    logic      is_wfi;
    logic      is_fence_i;
  } ctrl_t;

  // Privilege modes.
  localparam logic [1:0] PRV_U = 2'b00;
  localparam logic [1:0] PRV_M = 2'b11;

  // Exception cause codes.
  localparam logic [4:0] EXC_INSN_MISALIGNED  = 5'd0;
  localparam logic [4:0] EXC_INSN_ACCESS      = 5'd1;
  localparam logic [4:0] EXC_ILLEGAL          = 5'd2;
  localparam logic [4:0] EXC_BREAKPOINT       = 5'd3;
  localparam logic [4:0] EXC_LOAD_MISALIGNED  = 5'd4;
  localparam logic [4:0] EXC_LOAD_ACCESS      = 5'd5;
  localparam logic [4:0] EXC_STORE_MISALIGNED = 5'd6;
  localparam logic [4:0] EXC_STORE_ACCESS     = 5'd7;
  localparam logic [4:0] EXC_ECALL_U          = 5'd8;
  localparam logic [4:0] EXC_ECALL_M          = 5'd11;

  // Interrupt cause codes (mcause[31] = 1).
  localparam logic [4:0] IRQ_MSI = 5'd3;
  localparam logic [4:0] IRQ_MTI = 5'd7;
  localparam logic [4:0] IRQ_MEI = 5'd11;

  // CSR addresses.
  localparam logic [11:0] CSR_MSTATUS    = 12'h300;
  localparam logic [11:0] CSR_MISA       = 12'h301;
  localparam logic [11:0] CSR_MIE        = 12'h304;
  localparam logic [11:0] CSR_MTVEC      = 12'h305;
  localparam logic [11:0] CSR_MCOUNTEREN = 12'h306;
  localparam logic [11:0] CSR_MENVCFG    = 12'h30A;
  localparam logic [11:0] CSR_MSTATUSH   = 12'h310;
  localparam logic [11:0] CSR_MENVCFGH   = 12'h31A;
  localparam logic [11:0] CSR_MCOUNTINH  = 12'h320;
  localparam logic [11:0] CSR_MSCRATCH   = 12'h340;
  localparam logic [11:0] CSR_MEPC       = 12'h341;
  localparam logic [11:0] CSR_MCAUSE     = 12'h342;
  localparam logic [11:0] CSR_MTVAL      = 12'h343;
  localparam logic [11:0] CSR_MIP        = 12'h344;
  localparam logic [11:0] CSR_PMPCFG0    = 12'h3A0;
  localparam logic [11:0] CSR_PMPADDR0   = 12'h3B0;
  localparam logic [11:0] CSR_MCYCLE     = 12'hB00;
  localparam logic [11:0] CSR_MINSTRET   = 12'hB02;
  localparam logic [11:0] CSR_MCYCLEH    = 12'hB80;
  localparam logic [11:0] CSR_MINSTRETH  = 12'hB82;
  localparam logic [11:0] CSR_CYCLE      = 12'hC00;
  localparam logic [11:0] CSR_TIME       = 12'hC01;
  localparam logic [11:0] CSR_INSTRET    = 12'hC02;
  localparam logic [11:0] CSR_CYCLEH     = 12'hC80;
  localparam logic [11:0] CSR_TIMEH      = 12'hC81;
  localparam logic [11:0] CSR_INSTRETH   = 12'hC82;
  localparam logic [11:0] CSR_MVENDORID  = 12'hF11;
  localparam logic [11:0] CSR_MARCHID    = 12'hF12;
  localparam logic [11:0] CSR_MIMPID     = 12'hF13;
  localparam logic [11:0] CSR_MHARTID    = 12'hF14;
  localparam logic [11:0] CSR_MCONFIGPTR = 12'hF15;

  localparam logic [31:0] MISA_VALUE  = 32'h4010_0100;  // RV32, I, U
  localparam logic [31:0] MIMPID_VALUE = 32'h0001_0000;

  localparam logic [31:0] RESET_PC = 32'h0000_0000;

endpackage
