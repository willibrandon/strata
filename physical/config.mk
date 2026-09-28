# OpenROAD-flow-scripts configuration for the strata_soc layout (SkyWater
# sky130, sky130_fd_sc_hd). Synthesis is skipped: the input is the gate-level
# netlist that the gate simulator runs (build/gate/strata_soc.v, from
# scripts/synth_gate.py). Paths are inside the container, where the
# repository is mounted at /work (see scripts/physical.py).
export DESIGN_NAME     = strata_soc
export DESIGN_NICKNAME = strata_soc
export PLATFORM        = sky130hd

export SYNTH_NETLIST_FILES = /work/build/gate/strata_soc.v
export SDC_FILE            = /work/physical/strata_soc.sdc

export CORE_UTILIZATION = 45
export CORE_ASPECT_RATIO = 1
export CORE_MARGIN = 4

# Repair max-slew and max-capacitance violations with 20% margin: routing
# can lengthen a net after repair_design has run, and the Liberty tables end
# at 1.5 ns input transition.
export SLEW_MARGIN = 20
export CAP_MARGIN = 20
