// riscv-formal wrapper for the S1 core.
//
// The environment answers both buses with arbitrary data after arbitrary
// delays, following the bus protocol (docs/bus.md): a response only for an
// outstanding command, at least one cycle later. Interrupt lines and mtime
// are arbitrary. The wrapper also asserts that the core obeys the protocol.
//
// Constraints (STRATA_FV_*), documented in docs/verification.md:
//   * no bus error responses unless STRATA_FV_BUS_ERRORS is defined;
//   * all instructions execute in M-mode, loads and stores use M-mode
//     permissions and no PMP entry is locked. The riscv-formal instruction
//     models do not include privilege or PMP; those are covered by directed
//     tests.
module rvfi_wrapper (
  input clock,
  input reset,
  `RVFI_OUTPUTS
);

  (* keep *) `rvformal_rand_reg        ibus_ready_r;
  (* keep *) `rvformal_rand_reg        ibus_rsp_r;
  (* keep *) `rvformal_rand_reg [31:0] ibus_rdata;
  (* keep *) `rvformal_rand_reg        ibus_err_r;
  (* keep *) `rvformal_rand_reg        dbus_ready_r;
  (* keep *) `rvformal_rand_reg        dbus_rsp_r;
  (* keep *) `rvformal_rand_reg [31:0] dbus_rdata;
  (* keep *) `rvformal_rand_reg        dbus_err_r;
  (* keep *) `rvformal_rand_reg [2:0]  irq;
  (* keep *) `rvformal_rand_reg [63:0] mtime;

  logic        ibus_cmd_valid, dbus_cmd_valid, dbus_cmd_we;
  logic [31:0] ibus_cmd_addr, dbus_cmd_addr, dbus_cmd_wdata;
  logic [3:0]  dbus_cmd_be;
  logic        ipend, dpend;
  logic [1:0]  fv_data_mode;
  logic        fv_pmp_locked;

  wire ibus_rsp_valid = ipend && ibus_rsp_r;
  wire dbus_rsp_valid = dpend && dbus_rsp_r;

`ifdef STRATA_FV_BUS_ERRORS
  wire ibus_err = ibus_err_r;
  wire dbus_err = dbus_err_r;
`else
  wire ibus_err = 1'b0;
  wire dbus_err = 1'b0;
`endif

  always @(posedge clock) begin
    if (reset) begin
      ipend <= 1'b0;
      dpend <= 1'b0;
    end else begin
      if (ibus_rsp_valid) ipend <= 1'b0;
      if (ibus_cmd_valid && ibus_ready_r) ipend <= 1'b1;
      if (dbus_rsp_valid) dpend <= 1'b0;
      if (dbus_cmd_valid && dbus_ready_r) dpend <= 1'b1;
    end
  end

  // Bus protocol: at most one outstanding command per master.
  always @* if (!reset) begin
    if (ipend && !ibus_rsp_valid) assert(!ibus_cmd_valid);
    if (dpend && !dbus_rsp_valid) assert(!dbus_cmd_valid);
  end

  // Environment constraints.
  always @* if (!reset) begin
    if (rvfi_valid) assume(rvfi_mode == 2'b11);
    assume(fv_data_mode == 2'b11);
    assume(!fv_pmp_locked);
  end

  st_core uut (
    .clk           (clock),
    .rst           (reset),
    .halt          (1'b0),
    .mtime         (mtime),
    .irq_msip      (irq[0]),
    .irq_mtip      (irq[1]),
    .irq_meip      (irq[2]),
    .ibus_cmd_valid(ibus_cmd_valid),
    .ibus_cmd_addr (ibus_cmd_addr),
    .ibus_cmd_ready(ibus_ready_r),
    .ibus_rsp_valid(ibus_rsp_valid),
    .ibus_rsp_rdata(ibus_rdata),
    .ibus_rsp_err  (ibus_err),
    .dbus_cmd_valid(dbus_cmd_valid),
    .dbus_cmd_addr (dbus_cmd_addr),
    .dbus_cmd_we   (dbus_cmd_we),
    .dbus_cmd_be   (dbus_cmd_be),
    .dbus_cmd_wdata(dbus_cmd_wdata),
    .dbus_cmd_ready(dbus_ready_r),
    .dbus_rsp_valid(dbus_rsp_valid),
    .dbus_rsp_rdata(dbus_rdata),
    .dbus_rsp_err  (dbus_err),
    .trace_valid(), .trace_trap(), .trace_intr(), .trace_cause(), .trace_pc(),
    .trace_insn(), .trace_next_pc(), .trace_mode(), .trace_rd_we(), .trace_rd(),
    .trace_rd_wdata(),
    `RVFI_CONN,
    .fv_data_mode  (fv_data_mode),
    .fv_pmp_locked (fv_pmp_locked)
  );

endmodule
