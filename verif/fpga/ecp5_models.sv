// Behavioral models of the ECP5 primitives that rtl/fpga instantiates, for
// simulating the board top (testbench only; synthesis uses the real cells).
//
// The models cover only the configuration Strata uses:
//   EHXPLLL  feedback from CLKOP, outputs CLKOP and CLKOS started in phase
//            once LOCK rises (CPHASE/FPHASE and the loop dynamics are not
//            modeled);
//   ODDRX1F  D0 and D1 registered on the rising edge of SCLK, D0 driven while
//            SCLK is high and D1 while it is low;
//   USRMCLK  no behavior: the testbench reads the flash clock at the
//            primitive's input.
`timescale 1ns/1ps

module EHXPLLL #(
  parameter CLKI_DIV = 1, CLKFB_DIV = 1,
  parameter CLKOP_DIV = 8, CLKOS_DIV = 8, CLKOS2_DIV = 8, CLKOS3_DIV = 8,
  parameter CLKOP_ENABLE = "ENABLED", CLKOS_ENABLE = "DISABLED",
  parameter CLKOS2_ENABLE = "DISABLED", CLKOS3_ENABLE = "DISABLED",
  parameter CLKOP_CPHASE = 0, CLKOS_CPHASE = 0, CLKOS2_CPHASE = 0, CLKOS3_CPHASE = 0,
  parameter CLKOP_FPHASE = 0, CLKOS_FPHASE = 0, CLKOS2_FPHASE = 0, CLKOS3_FPHASE = 0,
  parameter FEEDBK_PATH = "CLKOP",
  parameter PLLRST_ENA = "DISABLED", INTFB_WAKE = "DISABLED", STDBY_ENABLE = "DISABLED",
  parameter DPHASE_SOURCE = "DISABLED",
  parameter OUTDIVIDER_MUXA = "DIVA", OUTDIVIDER_MUXB = "DIVB",
  parameter OUTDIVIDER_MUXC = "DIVC", OUTDIVIDER_MUXD = "DIVD",
  parameter LOCK_CYCLES = 64
) (
  input  logic CLKI, CLKFB,
  input  logic PHASESEL1, PHASESEL0, PHASEDIR, PHASESTEP, PHASELOADREG,
  input  logic STDBY, PLLWAKESYNC, RST,
  input  logic ENCLKOP, ENCLKOS, ENCLKOS2, ENCLKOS3,
  output logic CLKOP, CLKOS, CLKOS2, CLKOS3,
  output logic LOCK, INTLOCK, REFCLK, CLKINTFB
);

  realtime t0, t_in, t_op, t_os;

  assign CLKOS2 = 1'b0;
  assign CLKOS3 = 1'b0;
  assign INTLOCK = LOCK;
  assign REFCLK = CLKI;
  assign CLKINTFB = 1'b0;

  initial begin
    if (FEEDBK_PATH != "CLKOP") $fatal(1, "EHXPLLL model: only FEEDBK_PATH=CLKOP is modeled");
    CLKOP = 1'b0;
    CLKOS = 1'b0;
    LOCK = 1'b0;
    @(posedge CLKI) t0 = $realtime;
    @(posedge CLKI) t_in = $realtime - t0;
    t_op = t_in * CLKI_DIV / CLKFB_DIV;
    t_os = t_op * CLKOS_DIV / CLKOP_DIV;
    repeat (LOCK_CYCLES) @(posedge CLKI);
    LOCK = 1'b1;
    fork
      forever begin CLKOP = 1'b1; #(t_op / 2); CLKOP = 1'b0; #(t_op / 2); end
      forever begin CLKOS = 1'b1; #(t_os / 2); CLKOS = 1'b0; #(t_os / 2); end
    join
  end

endmodule

module ODDRX1F (
  input  logic D0,
  input  logic D1,
  input  logic SCLK,
  input  logic RST,
  output logic Q
);
  logic d0_q = 1'b0, d1_q = 1'b0;
  always @(posedge SCLK) begin
    d0_q <= D0;
    d1_q <= D1;
  end
  assign Q = SCLK ? d0_q : d1_q;
endmodule

module USRMCLK (
  input logic USRMCLKI,
  input logic USRMCLKTS
);
endmodule
