// Strata S1 core: control and status registers, privilege state, trap entry
// and return, counters, and interrupt selection.
//
// All CSR accesses and privilege changes happen in the W stage, which is the
// pipeline's single commit point. Any CSR write, trap, or MRET is followed by
// a pipeline flush (done by st_core), so earlier stages always observe
// committed CSR state.
module st_csr
  import st_pkg::*;
(
  input  logic        clk,
  input  logic        rst,

  // Platform inputs.
  input  logic [63:0] mtime,
  input  logic        irq_msip,
  input  logic        irq_mtip,
  input  logic        irq_meip,

  // CSR access by the instruction in W (combinational check and read).
  input  logic [11:0] csr_addr,
  input  logic        csr_write,     // instruction writes the CSR
  output logic        csr_illegal,
  output logic [31:0] csr_rdata,
  // Commit the access: write csr_wdata if csr_write.
  input  logic        csr_commit,
  input  csr_op_e     csr_op,
  input  logic [31:0] csr_operand,   // rs1 value or zero-extended immediate

  // Trap entry / return, and retirement (mutually exclusive in one cycle).
  input  logic        trap_commit,
  input  logic        trap_intr,
  input  logic [4:0]  trap_code,
  input  logic [31:0] trap_pc,
  input  logic [31:0] trap_tval,
  input  logic        mret_commit,
  input  logic        retire,        // an instruction retired without trapping

  // State for the rest of the core.
  output logic [1:0]  mode,
  output logic        mstatus_mprv,
  output logic [1:0]  mstatus_mpp,
  output logic        mstatus_tw,
  output logic [31:0] trap_vector,   // handler address for (trap_intr, trap_code)
  output logic [31:0] mepc_out,
  output logic        irq_req,       // an interrupt should be taken
  output logic [4:0]  irq_code,
  output logic        wfi_wake,      // (mip & mie) != 0
  output logic [31:0] pmp_cfg,
  output logic [127:0] pmp_addr
);

  // ---------------------------------------------------------------- state
  logic [1:0]  mode_q;
  logic        mie_q, mpie_q, mpp_m_q, mprv_q, tw_q;   // mstatus fields
  logic        msie_q, mtie_q, meie_q;                  // mie
  logic [31:2] mtvec_base_q;
  logic        mtvec_vec_q;
  logic [2:0]  mcounteren_q;
  logic [31:0] mscratch_q;
  logic [31:2] mepc_q;
  logic        mcause_int_q;
  logic [4:0]  mcause_code_q;
  logic [31:0] mtval_q;
  logic [7:0]  pmpcfg_q  [4];
  logic [31:0] pmpaddr_q [4];
  logic [63:0] mcycle_q, minstret_q;

  // ---------------------------------------------------------------- views
  logic [31:0] mstatus, mip, mie, mtvec, mcause;
  assign mstatus = {10'b0, tw_q, 3'b0, mprv_q, 4'b0, mpp_m_q, mpp_m_q, 3'b0, mpie_q, 3'b0, mie_q, 3'b0};
  assign mip     = {20'b0, irq_meip, 3'b0, irq_mtip, 3'b0, irq_msip, 3'b0};
  assign mie     = {20'b0, meie_q, 3'b0, mtie_q, 3'b0, msie_q, 3'b0};
  assign mtvec   = {mtvec_base_q, 1'b0, mtvec_vec_q};
  assign mcause  = {mcause_int_q, 26'b0, mcause_code_q};

  assign mode         = mode_q;
  assign mstatus_mprv = mprv_q;
  assign mstatus_mpp  = mpp_m_q ? PRV_M : PRV_U;
  assign mstatus_tw   = tw_q;
  assign mepc_out     = {mepc_q, 2'b00};

  for (genvar i = 0; i < 4; i++) begin : gen_pmp_out
    assign pmp_cfg[8*i +: 8]    = pmpcfg_q[i];
    assign pmp_addr[32*i +: 32] = pmpaddr_q[i];
  end

  // ---------------------------------------------------------------- interrupts
  logic [31:0] pending;
  assign pending  = mip & mie;
  assign wfi_wake = |pending;
  assign irq_req  = ((mode_q == PRV_U) || mie_q) && |pending;
  always_comb begin
    if (pending[11])     irq_code = IRQ_MEI;
    else if (pending[3]) irq_code = IRQ_MSI;
    else                 irq_code = IRQ_MTI;
  end

  assign trap_vector = (mtvec_vec_q && trap_intr) ? {mtvec_base_q + {25'b0, trap_code}, 2'b00}
                                                  : {mtvec_base_q, 2'b00};

  // ---------------------------------------------------------------- CSR read / legality
  logic exists;
  logic counter_ok;   // U-mode counter access permitted by mcounteren

  always_comb begin
    exists     = 1'b1;
    counter_ok = 1'b1;
    csr_rdata  = 32'd0;
    unique casez (csr_addr)
      CSR_MSTATUS:    csr_rdata = mstatus;
      CSR_MISA:       csr_rdata = MISA_VALUE;
      CSR_MIE:        csr_rdata = mie;
      CSR_MTVEC:      csr_rdata = mtvec;
      CSR_MCOUNTEREN: csr_rdata = {29'b0, mcounteren_q};
      CSR_MENVCFG, CSR_MSTATUSH, CSR_MENVCFGH, CSR_MCOUNTINH: csr_rdata = 32'd0;
      CSR_MSCRATCH:   csr_rdata = mscratch_q;
      CSR_MEPC:       csr_rdata = {mepc_q, 2'b00};
      CSR_MCAUSE:     csr_rdata = mcause;
      CSR_MTVAL:      csr_rdata = mtval_q;
      CSR_MIP:        csr_rdata = mip;
      CSR_PMPCFG0:    csr_rdata = {pmpcfg_q[3], pmpcfg_q[2], pmpcfg_q[1], pmpcfg_q[0]};
      12'h3A1, 12'h3A2, 12'h3A3: csr_rdata = 32'd0;
      12'b0011_1011_00??: csr_rdata = pmpaddr_q[csr_addr[1:0]];   // pmpaddr0-3
      12'b0011_1011_01??, 12'b0011_1011_1???: csr_rdata = 32'd0;  // pmpaddr4-15
      CSR_MCYCLE,   CSR_CYCLE:    csr_rdata = mcycle_q[31:0];
      CSR_MCYCLEH,  CSR_CYCLEH:   csr_rdata = mcycle_q[63:32];
      CSR_MINSTRET, CSR_INSTRET:  csr_rdata = minstret_q[31:0];
      CSR_MINSTRETH, CSR_INSTRETH: csr_rdata = minstret_q[63:32];
      CSR_TIME:       csr_rdata = mtime[31:0];
      CSR_TIMEH:      csr_rdata = mtime[63:32];
      CSR_MVENDORID, CSR_MARCHID, CSR_MHARTID, CSR_MCONFIGPTR: csr_rdata = 32'd0;
      CSR_MIMPID:     csr_rdata = MIMPID_VALUE;
      default: begin
        // Read-only-zero ranges: mhpmevent3-31, mhpmcounter3-31(h), hpmcounter3-31(h).
        exists = ((csr_addr >= 12'h323) && (csr_addr <= 12'h33F)) ||
                 ((csr_addr >= 12'hB03) && (csr_addr <= 12'hB1F)) ||
                 ((csr_addr >= 12'hB83) && (csr_addr <= 12'hB9F)) ||
                 ((csr_addr >= 12'hC03) && (csr_addr <= 12'hC1F)) ||
                 ((csr_addr >= 12'hC83) && (csr_addr <= 12'hC9F));
      end
    endcase

    // U-mode counter access (0xC00-0xC1F, 0xC80-0xC9F) needs mcounteren[n].
    if (csr_addr[11:8] == 4'hC && csr_addr[6:5] == 2'b00) begin
      unique case (csr_addr[4:0])
        5'd0:    counter_ok = mcounteren_q[0];
        5'd1:    counter_ok = mcounteren_q[1];
        5'd2:    counter_ok = mcounteren_q[2];
        default: counter_ok = 1'b0;
      endcase
    end
  end

  assign csr_illegal = !exists ||
                       ((mode_q != PRV_M) && (csr_addr[9:8] != 2'b00)) ||
                       (csr_write && (csr_addr[11:10] == 2'b11)) ||
                       ((mode_q != PRV_M) && !counter_ok);

  logic [31:0] wdata;
  always_comb begin
    unique case (csr_op)
      CSR_RS:  wdata = csr_rdata | csr_operand;
      CSR_RC:  wdata = csr_rdata & ~csr_operand;
      default: wdata = csr_operand;
    endcase
  end

  logic do_write;
  assign do_write = csr_commit && csr_write;

  // A locked entry, or the TOR base of a locked TOR entry, ignores writes.
  logic [3:0] pmpaddr_locked;
  for (genvar i = 0; i < 4; i++) begin : gen_pmp_lock
    if (i < 3) begin : g_next
      assign pmpaddr_locked[i] = pmpcfg_q[i][7] ||
                                 (pmpcfg_q[i+1][7] && pmpcfg_q[i+1][4:3] == 2'd1);
    end else begin : g_last
      assign pmpaddr_locked[i] = pmpcfg_q[i][7];
    end
  end

  // ---------------------------------------------------------------- update
  always_ff @(posedge clk) begin
    if (rst) begin
      mode_q        <= PRV_M;
      mie_q         <= 1'b0;
      mpie_q        <= 1'b0;
      mpp_m_q       <= 1'b0;
      mprv_q        <= 1'b0;
      tw_q          <= 1'b0;
      msie_q        <= 1'b0;
      mtie_q        <= 1'b0;
      meie_q        <= 1'b0;
      mtvec_base_q  <= '0;
      mtvec_vec_q   <= 1'b0;
      mcounteren_q  <= '0;
      mcause_int_q  <= 1'b0;
      mcause_code_q <= '0;
      mcycle_q      <= '0;
      minstret_q    <= '0;
      for (int i = 0; i < 4; i++) begin
        pmpcfg_q[i]  <= '0;
        pmpaddr_q[i] <= '0;
      end
    end else begin
      // Counters (a write to a half replaces it and suppresses the increment).
      mcycle_q <= mcycle_q + 64'd1;
      if (retire) minstret_q <= minstret_q + 64'd1;

      if (trap_commit) begin
        mepc_q        <= trap_pc[31:2];
        mcause_int_q  <= trap_intr;
        mcause_code_q <= trap_code;
        mtval_q       <= trap_tval;
        mpie_q        <= mie_q;
        mie_q         <= 1'b0;
        mpp_m_q       <= (mode_q == PRV_M);
        mode_q        <= PRV_M;
      end else if (mret_commit) begin
        mode_q  <= mpp_m_q ? PRV_M : PRV_U;
        mie_q   <= mpie_q;
        mpie_q  <= 1'b1;
        mpp_m_q <= 1'b0;
        if (!mpp_m_q) mprv_q <= 1'b0;
      end else if (do_write) begin
        unique casez (csr_addr)
          CSR_MSTATUS: begin
            mie_q   <= wdata[3];
            mpie_q  <= wdata[7];
            mpp_m_q <= (wdata[12:11] == PRV_M);
            mprv_q  <= wdata[17];
            tw_q    <= wdata[21];
          end
          CSR_MIE: begin
            msie_q <= wdata[3];
            mtie_q <= wdata[7];
            meie_q <= wdata[11];
          end
          CSR_MTVEC: begin
            mtvec_base_q <= wdata[31:2];
            mtvec_vec_q  <= wdata[0];
          end
          CSR_MCOUNTEREN: mcounteren_q <= wdata[2:0];
          CSR_MSCRATCH:   mscratch_q   <= wdata;
          CSR_MEPC:       mepc_q       <= wdata[31:2];
          CSR_MCAUSE: begin
            mcause_int_q  <= wdata[31];
            mcause_code_q <= wdata[4:0];
          end
          CSR_MTVAL:      mtval_q <= wdata;
          CSR_PMPCFG0: begin
            for (int i = 0; i < 4; i++) begin
              if (!pmpcfg_q[i][7]) begin
                // Bits [6:5] are reserved; R=0,W=1 is stored as W=0.
                pmpcfg_q[i] <= {wdata[8*i+7], 2'b00, wdata[8*i+3 +: 2], wdata[8*i+2],
                                wdata[8*i+1] & wdata[8*i], wdata[8*i]};
              end
            end
          end
          12'b0011_1011_00??: begin
            if (!pmpaddr_locked[csr_addr[1:0]]) pmpaddr_q[csr_addr[1:0]] <= wdata;
          end
          CSR_MCYCLE:    mcycle_q[31:0]    <= wdata;
          CSR_MCYCLEH:   mcycle_q[63:32]   <= wdata;
          CSR_MINSTRET:  minstret_q[31:0]  <= wdata;
          CSR_MINSTRETH: minstret_q[63:32] <= wdata;
          default: ;
        endcase
        // Writing one half of a counter replaces that half only.
        if (csr_addr == CSR_MCYCLE)    mcycle_q[63:32]   <= mcycle_q[63:32];
        if (csr_addr == CSR_MCYCLEH)   mcycle_q[31:0]    <= mcycle_q[31:0];
        if (csr_addr == CSR_MINSTRET)  minstret_q[63:32] <= minstret_q[63:32];
        if (csr_addr == CSR_MINSTRETH) minstret_q[31:0]  <= minstret_q[31:0];
      end
    end
  end

endmodule
