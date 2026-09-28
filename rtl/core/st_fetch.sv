// Strata S1 core: instruction fetch.
//
// Issues sequential fetches on the instruction bus and hands instructions to
// decode through a one-entry skid buffer. At most one fetch is outstanding.
// A redirect (taken branch, trap, return, or serialization) immediately
// issues a fetch of the new target when the bus allows it; any older fetch
// still in flight is discarded when it returns.
//
// Fetch permission (PMP execute check) is evaluated on the address of the
// outstanding fetch when its response arrives, so the check is off the
// address-issue path. Instruction-bus reads have no side effects, which makes
// issuing a fetch that later turns out to be forbidden harmless.
module st_fetch
  import st_pkg::*;
(
  input  logic        clk,
  input  logic        rst,

  input  logic        halt,          // stop issuing fetches

  input  logic        redirect,
  input  logic [31:0] redirect_pc,

  // Instruction offered to decode.
  output logic        out_valid,
  output logic [31:0] out_pc,
  output logic [31:0] out_insn,
  output logic        out_fault,     // instruction access fault
  input  logic        out_take,      // decode consumes it this cycle

  // PMP execute check of the returning fetch.
  output logic [31:0] chk_pc,
  input  logic        chk_ok,

  // Instruction bus.
  output logic        ibus_cmd_valid,
  output logic [31:0] ibus_cmd_addr,
  input  logic        ibus_cmd_ready,
  input  logic        ibus_rsp_valid,
  input  logic [31:0] ibus_rsp_rdata,
  input  logic        ibus_rsp_err
);

  logic [31:0] pc_q;          // next sequential fetch address
  logic        pend_q;        // a fetch is outstanding
  logic [31:0] pend_pc_q;     // its address
  logic        kill_q;        // discard its response (issued before a redirect)
  logic        skid_valid_q;
  logic [31:0] skid_pc_q;
  logic [31:0] skid_insn_q;
  logic        skid_fault_q;

  assign chk_pc = pend_pc_q;

  logic rsp_here;    // the outstanding fetch responds this cycle
  logic rsp_use;     // ... and is on the current path
  logic rsp_fault;
  assign rsp_here  = pend_q && ibus_rsp_valid;
  assign rsp_use   = rsp_here && !kill_q;
  assign rsp_fault = ibus_rsp_err || !chk_ok;

  assign out_valid = skid_valid_q || rsp_use;
  assign out_pc    = skid_valid_q ? skid_pc_q    : pend_pc_q;
  assign out_insn  = skid_valid_q ? skid_insn_q  : ibus_rsp_rdata;
  assign out_fault = skid_valid_q ? skid_fault_q : rsp_fault;

  logic take;
  assign take = out_valid && out_take && !redirect;

  // Instructions fetched but not consumed after this cycle. A new fetch is
  // issued on the sequential path only if its response will find the skid
  // buffer empty, which keeps the buffer at one entry.
  logic backlog;
  assign backlog = (skid_valid_q && !take) || (skid_valid_q && rsp_use) || (rsp_use && !take);

  logic bus_free;    // one-outstanding rule: previous fetch done or finishing now
  assign bus_free = !pend_q || rsp_here;

  assign ibus_cmd_valid = !halt && bus_free && (redirect || !backlog);
  assign ibus_cmd_addr  = redirect ? redirect_pc : pc_q;

  logic issue;
  assign issue = ibus_cmd_valid && ibus_cmd_ready;

  always_ff @(posedge clk) begin
    if (rst) begin
      pc_q         <= RESET_PC;
      pend_q       <= 1'b0;
      kill_q       <= 1'b0;
      skid_valid_q <= 1'b0;
    end else begin
      // Outstanding fetch tracking.
      if (issue) begin
        pend_q    <= 1'b1;
        pend_pc_q <= ibus_cmd_addr;
        kill_q    <= 1'b0;
        pc_q      <= ibus_cmd_addr + 32'd4;
      end else begin
        if (rsp_here) pend_q <= 1'b0;
        if (redirect) begin
          pc_q <= redirect_pc;
          if (pend_q && !rsp_here) kill_q <= 1'b1;
        end
      end

      // Skid buffer.
      if (redirect) begin
        skid_valid_q <= 1'b0;
      end else if (skid_valid_q) begin
        if (take) skid_valid_q <= rsp_use;
      end else begin
        skid_valid_q <= rsp_use && !take;
      end
      if (!skid_valid_q || take) begin
        skid_pc_q    <= pend_pc_q;
        skid_insn_q  <= ibus_rsp_rdata;
        skid_fault_q <= rsp_fault;
      end
    end
  end

endmodule
