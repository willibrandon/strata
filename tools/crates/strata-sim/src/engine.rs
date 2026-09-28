//! The interface between the host and a logic engine that implements
//! `strata_soc`: the gate-level netlist simulator in gate mode, or a compiled
//! RTL model in RTL mode. The engine decides everything the machine does; the
//! host only supplies pin inputs and memory-macro read data.

use crate::ports::{SocIn, SocOut};

pub trait Engine {
    /// Short identification, e.g. `gate:sky130_fd_sc_hd (14900 cells)`.
    fn describe(&self) -> String;

    /// Applies input values for the current cycle.
    fn set_inputs(&mut self, inputs: &SocIn);

    /// Settles combinational logic for the current inputs and state.
    fn eval(&mut self);

    /// Output values after `eval`.
    fn outputs(&self) -> SocOut;

    /// Rising clock edge: every flip-flop captures its next state. Inputs must
    /// not change between `eval` and `tick`.
    fn tick(&mut self);
}
