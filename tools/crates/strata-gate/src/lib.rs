//! Event-driven, cycle-based simulation of Strata gate-level netlists.
//!
//! The netlist is the sky130 standard-cell netlist produced by synthesis. Each
//! cell is evaluated from its Liberty function; each flip-flop holds one bit.
//! Nothing above the cell level (no instruction semantics, no RTL behavior)
//! is modeled here: in gate mode, this netlist alone determines execution.

pub mod compile;
pub mod sim;

pub use compile::{CompileError, Compiled, Driver};
pub use sim::{EvalMode, Force, GateSim, GateState, PowerOn, Stats};

use std::path::Path;
use std::sync::Arc;

use strata_netlist::{CellLibrary, Netlist};

/// Loads a Yosys JSON netlist and a Liberty library and compiles them.
pub fn load(netlist: &Path, liberty: &Path, top: &str) -> Result<Arc<Compiled>, String> {
    let lib = CellLibrary::load(liberty).map_err(|e| e.to_string())?;
    let nl = Netlist::load(netlist, top).map_err(|e| e.to_string())?;
    Compiled::new(nl, &lib, "clk").map(Arc::new).map_err(|e| e.to_string())
}
