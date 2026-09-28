# Timing constraints for the strata_soc block: one 25 MHz clock. Inputs and
# outputs (board pins and the memory macro ports) are budgeted 20% of the
# period outside the block.
current_design strata_soc

set clk_period 40.0
set io_budget [expr {$clk_period * 0.2}]

create_clock -name clk -period $clk_period [get_ports clk]
set_input_delay  $io_budget -clock clk [delete_from_list [all_inputs] [get_ports clk]]
set_output_delay $io_budget -clock clk [all_outputs]
