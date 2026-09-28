//! The placed and routed layout (strata-layout) tied to the gate-level
//! netlist: which layout component each simulated cell is, and the physical
//! load and input transition of a cell for transistor-level simulation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use strata_gate::Compiled;
use strata_layout::{Layout, Net};
use strata_netlist::CellLibrary;

pub struct Physical {
    pub layout: Layout,
    /// Layout component of each gate-level cell (None for the few cells the
    /// flow removed as dead logic).
    pub cell_comp: Vec<Option<u32>>,
    /// Gate-level cell of each layout component (None: added by the flow).
    pub comp_cell: Vec<Option<u32>>,
    pub dir: PathBuf,
}

/// Where the flow writes its results (scripts/physical.py).
pub const DEFAULT_DIR: &str = "build/physical/results/sky130hd/strata_soc/base";

impl Physical {
    /// Reads `6_final.def` (and `6_final.spef`) from a flow result directory.
    pub fn load(dir: &Path, pdk_root: &Path, c: &Compiled) -> Result<Physical, String> {
        let def = dir.join("6_final.def");
        if !def.exists() {
            return Err(format!("{} not found (run scripts/physical.py)", def.display()));
        }
        let lib = pdk_root.join("sky130A/libs.ref/sky130_fd_sc_hd");
        let tlef = lib.join("techlef/sky130_fd_sc_hd__nom.tlef");
        let lef = lib.join("lef/sky130_fd_sc_hd.lef");
        let spef = dir.join("6_final.spef");
        let layout = Layout::load(&def, &[&tlef, &lef], spef.exists().then_some(spef.as_path()))?;
        let mut cell_comp = vec![None; c.netlist.cells.len()];
        let mut comp_cell = vec![None; layout.components.len()];
        let index: HashMap<&str, usize> = layout.components.iter().enumerate().map(|(i, k)| (k.name.as_str(), i)).collect();
        for (i, cell) in c.netlist.cells.iter().enumerate() {
            if let Some(&k) = index.get(Layout::component_name(&cell.name).as_str()) {
                cell_comp[i] = Some(k as u32);
                comp_cell[k] = Some(i as u32);
            }
        }
        Ok(Physical { layout, cell_comp, comp_cell, dir: dir.to_path_buf() })
    }

    pub fn matched(&self) -> usize {
        self.cell_comp.iter().filter(|c| c.is_some()).count()
    }

    /// The routed net at `pin` of gate-level cell `cell`.
    pub fn net_of(&self, cell: u32, pin: &str) -> Option<&Net> {
        let comp = self.cell_comp[cell as usize]?;
        self.layout.net_at(&self.layout.components[comp as usize].name, pin)
    }

    /// Load on a routed net: extracted wire capacitance plus the input
    /// capacitance of every pin on it (pF).
    pub fn load_of(&self, lib: &CellLibrary, net: &Net) -> f64 {
        let mut pins = 0.0;
        for (comp, pin) in &net.pins {
            let Some(k) = self.layout.component(comp) else { continue };
            if let Some(def) = lib.cells.get(&k.cell) {
                if let Some(i) = def.input_index(pin) {
                    pins += def.input_caps[i];
                }
            }
        }
        net.cap.unwrap_or(0.0) + pins
    }

    /// Transition time at the pins of a routed net, from the Liberty tables of
    /// its driver at the net's load (ns); `rise` selects the edge.
    pub fn slew_of(&self, lib: &CellLibrary, net: &Net, rise: bool) -> Option<f64> {
        let (comp, pin) = net.pins.iter().find(|(comp, pin)| {
            self.layout.component(comp).and_then(|k| lib.cells.get(&k.cell)).is_some_and(|d| d.output_index(pin).is_some())
        })?;
        let cell = &self.layout.component(comp)?.cell;
        let load = self.load_of(lib, net);
        let arcs = strata_spice::timing::arcs(lib, cell, pin);
        // The driver's own input transition is unknown here; 0.1 ns is typical
        // inside the design.
        arcs.iter()
            .filter_map(|a| if rise { a.rise_transition.as_ref() } else { a.fall_transition.as_ref() })
            .map(|t| t.lookup(0.1, load))
            .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
    }
}
