//! Transistor-level analysis of the standard cells.
//!
//! The gate-level model evaluates each cell from the Boolean function in its
//! Liberty definition and ignores time within a cycle. This crate looks one
//! level further down: it simulates the transistor netlists that the PDK
//! provides for the cells (`libs.ref/sky130_fd_sc_hd/spice`) with ngspice and
//! the foundry's device models, and compares them with the Liberty models:
//!
//! * [`verify`]: the logic function of every cell type the netlist uses,
//!   for every input combination (flip-flops: clocked sequences);
//! * [`arc`]: propagation delay and output transition of a timing arc at a
//!   given input transition and load, next to the Liberty table values.
//!
//! Fidelity: typical corner (tt, 25 C, 1.8 V); the PDK's cell netlists are
//! schematic netlists without layout parasitics, so wiring inside the cell
//! is ideal; loads are lumped capacitors.

pub mod arc;
pub mod deck;
pub mod ngspice;
pub mod pdk;
pub mod timing;
pub mod verify;

use std::collections::HashMap;
use std::path::Path;

pub use deck::Models;
pub use pdk::{Mosfet, Pdk, Subckt};
use strata_netlist::CellLibrary;

/// Everything needed for analyses: PDK paths, the Liberty library and the
/// transistor netlists.
pub struct Analyzer {
    pub pdk: Pdk,
    pub lib: CellLibrary,
    pub cells: HashMap<String, Subckt>,
}

impl Analyzer {
    /// Loads the PDK's typical-corner Liberty library and cell netlists.
    pub fn new() -> Result<Analyzer, String> {
        let pdk = Pdk::locate()?;
        let lib = CellLibrary::load(&pdk.liberty()).map_err(|e| e.to_string())?;
        Self::with_library(pdk, lib)
    }

    pub fn with_library(pdk: Pdk, lib: CellLibrary) -> Result<Analyzer, String> {
        let cells = pdk.load_cells()?;
        Ok(Analyzer { pdk, lib, cells })
    }

    pub fn arc(&self, spec: &arc::ArcSpec, work: &Path) -> Result<arc::ArcResult, String> {
        arc::simulate(&self.pdk, &self.lib, &self.cells, spec, work, Models::Cells)
    }

    pub fn verify(&self, cells: &[String], work: &Path, models: Models) -> Result<Vec<verify::CellCheck>, String> {
        verify::check_cells(&self.pdk, &self.lib, &self.cells, cells, work, models)
    }
}
