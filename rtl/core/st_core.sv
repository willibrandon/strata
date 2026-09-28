// Strata S1: an original five-stage, in-order RV32I CPU with Zicsr, Zifencei,
// M/U privilege modes and PMP. See docs/microarchitecture.md.
//
//   F  fetch        st_fetch: issues instruction-bus reads, one-entry skid buffer
//   D  decode       decode, register read (with W bypass)
//   X  execute      forwarding, ALU, branch resolution/redirect, address
//                   generation, alignment and PMP checks, load-use interlock
//   M  memory       issues the data-bus command; interrupts are taken here
//   W  writeback    receives load data; the single commit point: register
//                   write, CSR access, trap entry, MRET, serialization
module st_core
  import st_pkg::*;
(
  input  logic        clk,
  input  logic        rst,
  input  logic        halt,          // freeze after the current bus response

  input  logic [63:0] mtime,
  input  logic        irq_msip,
  input  logic        irq_mtip,
  input  logic        irq_meip,

  // Instruction bus (read-only).
  output logic        ibus_cmd_valid,
  output logic [31:0] ibus_cmd_addr,
  input  logic        ibus_cmd_ready,
  input  logic        ibus_rsp_valid,
  input  logic [31:0] ibus_rsp_rdata,
  input  logic        ibus_rsp_err,

  // Data bus.
  output logic        dbus_cmd_valid,
  output logic [31:0] dbus_cmd_addr,
  output logic        dbus_cmd_we,
  output logic [3:0]  dbus_cmd_be,
  output logic [31:0] dbus_cmd_wdata,
  input  logic        dbus_cmd_ready,
  input  logic        dbus_rsp_valid,
  input  logic [31:0] dbus_rsp_rdata,
  input  logic        dbus_rsp_err,

  // Retirement trace: one entry per instruction leaving W.
  output logic        trace_valid,     // an instruction completed (retired or trapped)
  output logic        trace_trap,      // ... by taking a trap (then it did not retire)
  output logic        trace_intr,      // the trap is an interrupt
  output logic [4:0]  trace_cause,
  output logic [31:0] trace_pc,
  output logic [31:0] trace_insn,
  output logic [31:0] trace_next_pc,
  output logic [1:0]  trace_mode,      // privilege mode it executed in
  output logic        trace_rd_we,
  output logic [4:0]  trace_rd,
  output logic [31:0] trace_rd_wdata

`ifdef RISCV_FORMAL
  ,
  output logic        rvfi_valid,
  output logic [63:0] rvfi_order,
  output logic [31:0] rvfi_insn,
  output logic        rvfi_trap,
  output logic        rvfi_halt,
  output logic        rvfi_intr,
  output logic [1:0]  rvfi_mode,
  output logic [1:0]  rvfi_ixl,
  output logic [4:0]  rvfi_rs1_addr,
  output logic [4:0]  rvfi_rs2_addr,
  output logic [31:0] rvfi_rs1_rdata,
  output logic [31:0] rvfi_rs2_rdata,
  output logic [4:0]  rvfi_rd_addr,
  output logic [31:0] rvfi_rd_wdata,
  output logic [31:0] rvfi_pc_rdata,
  output logic [31:0] rvfi_pc_wdata,
  output logic [31:0] rvfi_mem_addr,
  output logic [3:0]  rvfi_mem_rmask,
  output logic [3:0]  rvfi_mem_wmask,
  output logic [31:0] rvfi_mem_rdata,
  output logic [31:0] rvfi_mem_wdata
`endif
);

  // ==================================================================
  // Pipeline registers
  // ==================================================================
  // X
  logic        x_valid;
  logic [31:0] x_pc, x_insn, x_imm, x_rs1_val, x_rs2_val;
  ctrl_t       x_ctrl;
  logic [4:0]  x_rs1, x_rs2, x_rd;
  logic        x_exc;
  logic [4:0]  x_exc_cause;
  logic [31:0] x_exc_tval;
  // M
  logic        m_valid;
  logic [31:0] m_pc, m_insn, m_result, m_wdata, m_csr_operand, m_npc;
  logic [3:0]  m_be;
  ctrl_t       m_ctrl;
  logic [4:0]  m_rd;
  logic        m_exc;
  logic [4:0]  m_exc_cause;
  logic [31:0] m_exc_tval;
  // W
  logic        w_valid;
  logic [31:0] w_pc, w_insn, w_result, w_csr_operand, w_npc;
  ctrl_t       w_ctrl;
  logic [4:0]  w_rd;
  logic        w_exc;
  logic [4:0]  w_exc_cause;
  logic [31:0] w_exc_tval;
  logic        w_intr;         // this entry carries an interrupt taken in M
  logic [4:0]  w_intr_code;
  logic        w_pend;         // its data-bus command was issued; response pending

  // ==================================================================
  // Global control signals (defined further below)
  // ==================================================================
  logic        w_done, w_flush, w_ready_in;
  logic [31:0] w_redirect_pc, w_wb_data;
  logic        w_rf_we;
  logic        x_redirect;
  logic [31:0] x_target;
  logic        x_go, x_ready_in, m_go, m_ready_in, d_go;

  // ==================================================================
  // CSR unit and privilege state
  // ==================================================================
  logic [1:0]   mode, mpp, eff_mode;
  logic         mprv, tw, irq_req, wfi_wake;
  logic [4:0]   irq_code;
  logic [31:0]  trap_vector, mepc, csr_rdata;
  logic         csr_illegal;
  logic [31:0]  pmp_cfg;
  logic [127:0] pmp_addr;

  logic         trap_take, trap_intr, mret_commit, csr_commit, retire;
  logic [4:0]   trap_code;
  logic [31:0]  trap_tval;

  st_csr u_csr (
    .clk, .rst,
    .mtime, .irq_msip, .irq_mtip, .irq_meip,
    .csr_addr    (w_insn[31:20]),
    .csr_write   (w_ctrl.csr_write),
    .csr_illegal (csr_illegal),
    .csr_rdata   (csr_rdata),
    .csr_commit  (csr_commit),
    .csr_op      (w_ctrl.csr_op),
    .csr_operand (w_csr_operand),
    .trap_commit (trap_take),
    .trap_intr   (trap_intr),
    .trap_code   (trap_code),
    .trap_pc     (w_pc),
    .trap_tval   (trap_tval),
    .mret_commit (mret_commit),
    .retire      (retire),
    .mode, .mstatus_mprv(mprv), .mstatus_mpp(mpp), .mstatus_tw(tw),
    .trap_vector, .mepc_out(mepc),
    .irq_req, .irq_code, .wfi_wake,
    .pmp_cfg, .pmp_addr
  );

  // Loads and stores are checked with MPP when MPRV is set in M-mode.
  assign eff_mode = (mprv && mode == PRV_M) ? mpp : mode;

  // ==================================================================
  // F: fetch
  // ==================================================================
  logic        f_valid, f_fault, f_chk_ok;
  logic [31:0] f_pc, f_insn, f_chk_pc;
  logic        redirect;
  logic [31:0] redirect_pc;

  assign redirect    = w_flush || x_redirect;
  assign redirect_pc = w_flush ? w_redirect_pc : x_target;

  st_fetch u_fetch (
    .clk, .rst, .halt,
    .redirect, .redirect_pc,
    .out_valid (f_valid), .out_pc(f_pc), .out_insn(f_insn), .out_fault(f_fault),
    .out_take  (d_go),
    .chk_pc    (f_chk_pc), .chk_ok(f_chk_ok),
    .ibus_cmd_valid, .ibus_cmd_addr, .ibus_cmd_ready,
    .ibus_rsp_valid, .ibus_rsp_rdata, .ibus_rsp_err
  );

  st_pmp u_pmp_fetch (
    .cfg(pmp_cfg), .addr(pmp_addr), .mode(mode), .paddr(f_chk_pc),
    .acc_r(1'b0), .acc_w(1'b0), .acc_x(1'b1), .ok(f_chk_ok)
  );

  // ==================================================================
  // D: decode and register read
  // ==================================================================
  ctrl_t       d_ctrl_raw, d_ctrl;
  logic [31:0] d_imm, d_rs1_val, d_rs2_val, rf_rd1, rf_rd2;
  logic [4:0]  d_rs1, d_rs2, d_rd;
  logic        d_exc;
  logic [4:0]  d_exc_cause;
  logic [31:0] d_exc_tval;

  st_decode u_decode (
    .insn(f_insn), .ctrl(d_ctrl_raw), .imm(d_imm), .rs1(d_rs1), .rs2(d_rs2), .rd(d_rd)
  );

  // A faulted fetch carries no instruction: suppress all its effects.
  assign d_ctrl      = f_fault ? '0 : d_ctrl_raw;
  assign d_exc       = f_fault || d_ctrl_raw.illegal;
  assign d_exc_cause = f_fault ? EXC_INSN_ACCESS : EXC_ILLEGAL;
  assign d_exc_tval  = f_fault ? f_pc : f_insn;

  st_regfile u_rf (
    .clk,
    .ra1(d_rs1), .rd1(rf_rd1),
    .ra2(d_rs2), .rd2(rf_rd2),
    .we(w_rf_we), .wa(w_rd), .wd(w_wb_data)
  );

  // Write-through: the value W writes this cycle is not yet in the array.
  assign d_rs1_val = (w_rf_we && w_rd == d_rs1) ? w_wb_data : rf_rd1;
  assign d_rs2_val = (w_rf_we && w_rd == d_rs2) ? w_wb_data : rf_rd2;

  assign d_go = f_valid && x_ready_in;

  // ==================================================================
  // X: execute
  // ==================================================================
  // Results produced late: loads (data arrives in W) and CSR reads (performed in W).
  logic m_late, w_load_waiting;
  assign m_late         = m_ctrl.is_load || (m_ctrl.csr_op != CSR_NONE);
  assign w_load_waiting = w_valid && w_pend && !dbus_rsp_valid;

  logic m_fwd1, m_fwd2, w_fwd1, w_fwd2;
  assign m_fwd1 = m_valid && m_ctrl.writes_rd && m_rd == x_rs1;
  assign m_fwd2 = m_valid && m_ctrl.writes_rd && m_rd == x_rs2;
  assign w_fwd1 = w_valid && w_ctrl.writes_rd && w_rd == x_rs1;
  assign w_fwd2 = w_valid && w_ctrl.writes_rd && w_rd == x_rs2;

  logic [31:0] x_a, x_b;   // rs1/rs2 after forwarding
  assign x_a = m_fwd1 ? m_result : w_fwd1 ? w_wb_data : x_rs1_val;
  assign x_b = m_fwd2 ? m_result : w_fwd2 ? w_wb_data : x_rs2_val;

  // Interlock: a needed operand is not available yet.
  logic x_hazard;
  assign x_hazard = x_valid && (
      (x_ctrl.uses_rs1 && (m_fwd1 ? m_late : (w_fwd1 && w_load_waiting))) ||
      (x_ctrl.uses_rs2 && (m_fwd2 ? m_late : (w_fwd2 && w_load_waiting))));

  // ALU
  logic [31:0] alu_a, alu_b, alu_y;
  always_comb begin
    unique case (x_ctrl.a_sel)
      A_PC:    alu_a = x_pc;
      A_ZERO:  alu_a = 32'd0;
      default: alu_a = x_a;
    endcase
  end
  assign alu_b = x_ctrl.b_imm ? x_imm : x_b;

  st_alu u_alu (.op(x_ctrl.alu_op), .a(alu_a), .b(alu_b), .y(alu_y));

  // Branches and jumps
  logic        br_eq, br_lt, br_ltu, br_cond, x_taken, x_target_misaligned;
  logic [31:0] x_link, x_target_sum;
  assign br_eq  = (x_a == x_b);
  assign br_lt  = $signed(x_a) < $signed(x_b);
  assign br_ltu = x_a < x_b;
  always_comb begin
    unique case (x_insn[14:12])
      3'b000:  br_cond = br_eq;
      3'b001:  br_cond = !br_eq;
      3'b100:  br_cond = br_lt;
      3'b101:  br_cond = !br_lt;
      3'b110:  br_cond = br_ltu;
      3'b111:  br_cond = !br_ltu;
      default: br_cond = 1'b0;
    endcase
  end
  assign x_taken             = x_ctrl.is_jal || x_ctrl.is_jalr || (x_ctrl.is_branch && br_cond);
  assign x_target_sum        = (x_ctrl.is_jalr ? x_a : x_pc) + x_imm;
  assign x_target            = {x_target_sum[31:1], x_target_sum[0] & !x_ctrl.is_jalr};
  assign x_target_misaligned = x_taken && x_target[1];
  assign x_link              = x_pc + 32'd4;
  assign x_redirect          = x_go && x_taken && !x_target_misaligned;

  // Memory access preparation
  logic        x_mem, x_misaligned, x_pmp_ok;
  logic [31:0] x_wdata;
  logic [3:0]  x_be;
  assign x_mem = x_ctrl.is_load || x_ctrl.is_store;
  always_comb begin
    unique case (x_ctrl.mem_size)
      SZ_B: begin
        x_misaligned = 1'b0;
        x_be         = 4'b0001 << alu_y[1:0];
        x_wdata      = {4{x_b[7:0]}};
      end
      SZ_H: begin
        x_misaligned = alu_y[0];
        x_be         = 4'b0011 << alu_y[1:0];
        x_wdata      = {2{x_b[15:0]}};
      end
      default: begin
        x_misaligned = alu_y[1:0] != 2'b00;
        x_be         = 4'b1111;
        x_wdata      = x_b;
      end
    endcase
  end

  st_pmp u_pmp_data (
    .cfg(pmp_cfg), .addr(pmp_addr), .mode(eff_mode), .paddr(alu_y),
    .acc_r(x_ctrl.is_load), .acc_w(x_ctrl.is_store), .acc_x(1'b0), .ok(x_pmp_ok)
  );

  // Exceptions raised in X (only if nothing earlier was raised).
  logic        x_exc_out;
  logic [4:0]  x_exc_cause_out;
  logic [31:0] x_exc_tval_out;
  always_comb begin
    x_exc_out       = x_exc;
    x_exc_cause_out = x_exc_cause;
    x_exc_tval_out  = x_exc_tval;
    if (!x_exc) begin
      if (x_target_misaligned) begin
        x_exc_out = 1'b1; x_exc_cause_out = EXC_INSN_MISALIGNED; x_exc_tval_out = x_target;
      end else if (x_mem && x_misaligned) begin
        x_exc_out = 1'b1; x_exc_tval_out = alu_y;
        x_exc_cause_out = x_ctrl.is_load ? EXC_LOAD_MISALIGNED : EXC_STORE_MISALIGNED;
      end else if (x_mem && !x_pmp_ok) begin
        x_exc_out = 1'b1; x_exc_tval_out = alu_y;
        x_exc_cause_out = x_ctrl.is_load ? EXC_LOAD_ACCESS : EXC_STORE_ACCESS;
      end
    end
  end

  assign x_go       = x_valid && m_ready_in && !x_hazard;
  assign x_ready_in = !x_valid || x_go;

  // ==================================================================
  // M: memory command, interrupt sampling
  // ==================================================================
  logic take_irq, m_issue;
  assign take_irq = m_valid && irq_req && w_ready_in && !w_flush && !halt;
  assign m_issue  = m_valid && (m_ctrl.is_load || m_ctrl.is_store) && !m_exc && !take_irq;

  // Issue only when the instruction moves to W in the same cycle, and only
  // once W's own access (if any) has completed without error: this keeps at
  // most one access outstanding and makes bus errors precise.
  assign dbus_cmd_valid = m_issue && w_ready_in && !w_flush && !halt;
  assign dbus_cmd_addr  = m_result;
  assign dbus_cmd_we    = m_ctrl.is_store;
  assign dbus_cmd_be    = m_be;
  assign dbus_cmd_wdata = m_wdata;

  assign m_go       = m_valid && w_ready_in && !halt && (!m_issue || dbus_cmd_ready);
  assign m_ready_in = !m_valid || m_go;

  // ==================================================================
  // W: writeback and commit
  // ==================================================================
  logic w_rsp, w_bus_fault, w_wait, w_wfi_wait, w_busy;
  assign w_rsp       = w_pend && dbus_rsp_valid;
  assign w_bus_fault = w_rsp && dbus_rsp_err;
  assign w_wait      = w_pend && !dbus_rsp_valid;

  // Exceptions discovered in W.
  logic w_mret_illegal, w_wfi_illegal, w_csr_illegal, w_sync_trap;
  assign w_mret_illegal = w_ctrl.is_mret && mode != PRV_M;
  assign w_wfi_illegal  = w_ctrl.is_wfi && mode == PRV_U && tw;
  assign w_csr_illegal  = (w_ctrl.csr_op != CSR_NONE) && csr_illegal;
  assign w_sync_trap    = w_exc || w_mret_illegal || w_wfi_illegal || w_csr_illegal ||
                          w_ctrl.is_ecall || w_ctrl.is_ebreak || w_bus_fault;

  assign w_wfi_wait = w_ctrl.is_wfi && !w_intr && !w_sync_trap && !wfi_wake;
  assign w_busy     = w_wait || w_wfi_wait || (halt && !w_rsp);
  assign w_done     = w_valid && !w_busy;
  assign w_ready_in = !w_valid || w_done;

  assign trap_take = w_done && (w_intr || w_sync_trap);
  assign trap_intr = w_intr;
  always_comb begin
    trap_code = EXC_ILLEGAL;
    trap_tval = w_insn;
    if (w_intr) begin
      trap_code = w_intr_code;
      trap_tval = 32'd0;
    end else if (w_exc) begin
      trap_code = w_exc_cause;
      trap_tval = w_exc_tval;
    end else if (w_mret_illegal || w_wfi_illegal || w_csr_illegal) begin
      trap_code = EXC_ILLEGAL;
      trap_tval = w_insn;
    end else if (w_ctrl.is_ecall) begin
      trap_code = (mode == PRV_U) ? EXC_ECALL_U : EXC_ECALL_M;
      trap_tval = 32'd0;
    end else if (w_ctrl.is_ebreak) begin
      trap_code = EXC_BREAKPOINT;
      trap_tval = w_pc;
    end else if (w_bus_fault) begin
      trap_code = w_ctrl.is_load ? EXC_LOAD_ACCESS : EXC_STORE_ACCESS;
      trap_tval = w_result;
    end
  end

  logic w_commit;   // completes without trapping
  assign w_commit    = w_done && !trap_take;
  assign retire      = w_commit;
  assign csr_commit  = w_commit && (w_ctrl.csr_op != CSR_NONE);
  assign mret_commit = w_commit && w_ctrl.is_mret;

  logic w_serialize;
  assign w_serialize = (w_ctrl.csr_op != CSR_NONE && w_ctrl.csr_write) || w_ctrl.is_fence_i;

  assign w_flush       = trap_take || mret_commit || (w_commit && w_serialize);
  assign w_redirect_pc = trap_take ? trap_vector : mret_commit ? mepc : w_pc + 32'd4;

  // Load data alignment and extension.
  logic [31:0] load_shifted, load_val;
  assign load_shifted = dbus_rsp_rdata >> {w_result[1:0], 3'b000};
  always_comb begin
    unique case (w_ctrl.mem_size)
      SZ_B:    load_val = w_ctrl.mem_unsigned ? {24'b0, load_shifted[7:0]}
                                              : {{24{load_shifted[7]}}, load_shifted[7:0]};
      SZ_H:    load_val = w_ctrl.mem_unsigned ? {16'b0, load_shifted[15:0]}
                                              : {{16{load_shifted[15]}}, load_shifted[15:0]};
      default: load_val = load_shifted;
    endcase
  end

  assign w_wb_data = w_ctrl.is_load ? load_val :
                     (w_ctrl.csr_op != CSR_NONE) ? csr_rdata : w_result;
  assign w_rf_we   = w_commit && w_ctrl.writes_rd;

  // ==================================================================
  // Pipeline register updates
  // ==================================================================
  always_ff @(posedge clk) begin
    if (rst) begin
      x_valid <= 1'b0;
      m_valid <= 1'b0;
      w_valid <= 1'b0;
      w_pend  <= 1'b0;
    end else begin
      // D -> X
      if (w_flush) begin
        x_valid <= 1'b0;
      end else if (x_ready_in) begin
        x_valid <= d_go && !x_redirect;
      end
      if (x_ready_in) begin
        x_pc        <= f_pc;
        x_insn      <= f_insn;
        x_ctrl      <= d_ctrl;
        x_imm       <= d_imm;
        x_rs1       <= d_rs1;
        x_rs2       <= d_rs2;
        x_rd        <= d_rd;
        x_rs1_val   <= d_rs1_val;
        x_rs2_val   <= d_rs2_val;
        x_exc       <= d_exc;
        x_exc_cause <= d_exc_cause;
        x_exc_tval  <= d_exc_tval;
      end else begin
        // Holding: keep operands current as producers retire.
        x_rs1_val <= x_a;
        x_rs2_val <= x_b;
      end

      // X -> M
      if (w_flush) begin
        m_valid <= 1'b0;
      end else if (m_ready_in) begin
        m_valid <= x_go;
      end
      if (m_ready_in) begin
        m_pc          <= x_pc;
        m_insn        <= x_insn;
        m_ctrl        <= x_ctrl;
        m_rd          <= x_rd;
        m_result      <= (x_ctrl.is_jal || x_ctrl.is_jalr) ? x_link : alu_y;
        m_wdata       <= x_wdata;
        m_be          <= x_be;
        m_csr_operand <= x_ctrl.csr_imm ? {27'b0, x_rs1} : x_a;
        m_npc         <= (x_taken && !x_target_misaligned) ? x_target : x_link;
        m_exc         <= x_exc_out;
        m_exc_cause   <= x_exc_cause_out;
        m_exc_tval    <= x_exc_tval_out;
      end

      // M -> W
      if (w_ready_in) begin
        w_valid       <= m_go && !w_flush;
        w_pend        <= dbus_cmd_valid && dbus_cmd_ready;
        w_pc          <= m_pc;
        w_insn        <= m_insn;
        w_ctrl        <= m_ctrl;
        w_rd          <= m_rd;
        w_result      <= m_result;
        w_csr_operand <= m_csr_operand;
        w_npc         <= m_npc;
        w_exc         <= m_exc;
        w_exc_cause   <= m_exc_cause;
        w_exc_tval    <= m_exc_tval;
        w_intr        <= take_irq;
        w_intr_code   <= irq_code;
      end else if (w_rsp) begin
        w_pend <= 1'b0;
      end
    end
  end

  // ==================================================================
  // Retirement trace
  // ==================================================================
  assign trace_valid    = w_done;
  assign trace_trap     = trap_take;
  assign trace_intr     = w_intr;
  assign trace_cause    = trap_code;
  assign trace_pc       = w_pc;
  assign trace_insn     = w_insn;
  assign trace_next_pc  = w_flush ? w_redirect_pc : w_npc;
  assign trace_mode     = mode;
  assign trace_rd_we    = w_rf_we;
  assign trace_rd       = w_rd;
  assign trace_rd_wdata = w_wb_data;

`ifdef RISCV_FORMAL
  // ==================================================================
  // RISC-V Formal Interface (verification only)
  // ==================================================================
  logic [31:0] m_rs1_rdata, m_rs2_rdata, w_rs1_rdata, w_rs2_rdata, w_mem_wdata;
  logic [3:0]  w_be;
  logic        rvfi_intr_next;
  always_ff @(posedge clk) begin
    if (m_ready_in) begin
      m_rs1_rdata <= x_ctrl.uses_rs1 ? x_a : 32'd0;
      m_rs2_rdata <= x_ctrl.uses_rs2 ? x_b : 32'd0;
    end
    if (w_ready_in) begin
      w_rs1_rdata <= m_rs1_rdata;
      w_rs2_rdata <= m_rs2_rdata;
      w_mem_wdata <= m_wdata;
      w_be        <= m_be;
    end
    if (rst) begin
      rvfi_order     <= 64'd0;
      rvfi_intr_next <= 1'b0;
    end else if (w_done) begin
      // An interrupt carrier is not an executed instruction and is not reported;
      // the next reported instruction is the first of the handler.
      if (!w_intr) rvfi_order <= rvfi_order + 64'd1;
      rvfi_intr_next <= trap_take;
    end
  end

  logic w_mem_ok;
  assign w_mem_ok = w_rsp && !dbus_rsp_err && !w_intr;

  assign rvfi_valid     = w_done && !w_intr;
  assign rvfi_insn      = w_insn;
  assign rvfi_trap      = trap_take;
  assign rvfi_halt      = 1'b0;
  assign rvfi_intr      = rvfi_intr_next;
  assign rvfi_mode      = mode;
  assign rvfi_ixl       = 2'd1;
  assign rvfi_rs1_addr  = w_ctrl.uses_rs1 ? w_insn[19:15] : 5'd0;
  assign rvfi_rs2_addr  = w_ctrl.uses_rs2 ? w_insn[24:20] : 5'd0;
  assign rvfi_rs1_rdata = w_rs1_rdata;
  assign rvfi_rs2_rdata = w_rs2_rdata;
  assign rvfi_rd_addr   = w_rf_we ? w_rd : 5'd0;
  assign rvfi_rd_wdata  = w_rf_we ? w_wb_data : 32'd0;
  assign rvfi_pc_rdata  = w_pc;
  assign rvfi_pc_wdata  = trace_next_pc;
  assign rvfi_mem_addr  = {w_result[31:2], 2'b00};
  assign rvfi_mem_rmask = (w_mem_ok && w_ctrl.is_load)  ? w_be : 4'b0;
  assign rvfi_mem_wmask = (w_mem_ok && w_ctrl.is_store) ? w_be : 4'b0;
  assign rvfi_mem_rdata = dbus_rsp_rdata;
  assign rvfi_mem_wdata = w_mem_wdata;
`endif

endmodule
