// ECP5 PLL: 25 MHz in; 125 MHz (TMDS bit clock) and 25 MHz (system and pixel
// clock) out, phase aligned. Parameters from Project Trellis' ecppll.
module ulx3s_pll (
  input  logic clk_in,
  output logic clk_bit,
  output logic clk_px,
  output logic locked
);

  (* FREQUENCY_PIN_CLKI="25" *)
  (* FREQUENCY_PIN_CLKOP="125" *)
  (* FREQUENCY_PIN_CLKOS="25" *)
  (* ICP_CURRENT="12" *) (* LPF_RESISTOR="8" *) (* MFG_ENABLE_FILTEROPAMP="1" *) (* MFG_GMCREF_SEL="2" *)
  EHXPLLL #(
    .PLLRST_ENA("DISABLED"), .INTFB_WAKE("DISABLED"), .STDBY_ENABLE("DISABLED"),
    .DPHASE_SOURCE("DISABLED"),
    .OUTDIVIDER_MUXA("DIVA"), .OUTDIVIDER_MUXB("DIVB"), .OUTDIVIDER_MUXC("DIVC"), .OUTDIVIDER_MUXD("DIVD"),
    .CLKI_DIV(1),
    .CLKOP_ENABLE("ENABLED"), .CLKOP_DIV(5),  .CLKOP_CPHASE(2), .CLKOP_FPHASE(0),
    .CLKOS_ENABLE("ENABLED"), .CLKOS_DIV(25), .CLKOS_CPHASE(2), .CLKOS_FPHASE(0),
    .FEEDBK_PATH("CLKOP"), .CLKFB_DIV(5)
  ) u_pll (
    .RST(1'b0), .STDBY(1'b0), .CLKI(clk_in), .CLKOP(clk_bit), .CLKOS(clk_px),
    .CLKOS2(), .CLKOS3(), .CLKFB(clk_bit), .CLKINTFB(),
    .PHASESEL0(1'b0), .PHASESEL1(1'b0), .PHASEDIR(1'b1), .PHASESTEP(1'b1), .PHASELOADREG(1'b1),
    .PLLWAKESYNC(1'b0), .ENCLKOP(1'b0), .LOCK(locked),
    .ENCLKOS(), .ENCLKOS2(), .ENCLKOS3(), .INTLOCK(), .REFCLK()
  );

endmodule
