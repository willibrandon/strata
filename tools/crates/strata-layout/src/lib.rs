//! The placed and routed layout of `strata_soc` from OpenROAD
//! (`physical/`, `scripts/physical.py`): cell placement, routed wires and
//! vias of every signal net, I/O pins, and extracted net capacitance.
//!
//! Instance names are the gate-level netlist's hierarchical cell names with
//! `/` between levels (`u_core/_c1878_`), so every simulated cell can be
//! found in the layout. Cells that the flow added (clock tree, repair
//! buffers, tie cells, antenna diodes, well taps, fillers) are also present.
//! Coordinates are in micrometres.

pub mod def;
pub mod lef;
pub mod spef;

use std::collections::HashMap;
use std::path::Path;

pub use lef::{Layer, Lef};

#[derive(Debug, Clone, Default)]
pub struct Component {
    pub name: String,
    pub cell: String,
    /// Lower-left corner and size (µm).
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub orient: String,
    /// Added by the flow with no logic function (fillers, well taps).
    pub physical_only: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Wire {
    /// Routing layer index (0 = li1).
    pub layer: u8,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub width: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Via {
    /// Lower routing layer of the via.
    pub lower: u8,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Net {
    pub name: String,
    /// (component, pin); the component is `PIN` for a top-level port.
    pub pins: Vec<(String, String)>,
    pub wires: Vec<Wire>,
    pub vias: Vec<Via>,
    /// Total extracted capacitance (pF), when a SPEF file was read.
    pub cap: Option<f64>,
}

impl Net {
    /// Routed wire length (µm).
    pub fn length(&self) -> f64 {
        self.wires.iter().map(|w| ((w.x1 - w.x0).abs() + (w.y1 - w.y0).abs()) as f64).sum()
    }
}

#[derive(Debug, Clone, Default)]
pub struct IoPin {
    pub name: String,
    pub net: String,
    pub layer: u8,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub design: String,
    /// DEF database units per micrometre.
    pub dbu: f64,
    /// Die area: x0, y0, x1, y1.
    pub die: [f64; 4],
    /// Placement rows: x0, y, x1, height.
    pub rows: Vec<[f64; 4]>,
    pub layers: Vec<Layer>,
    pub components: Vec<Component>,
    pub pins: Vec<IoPin>,
    pub nets: Vec<Net>,
    by_name: HashMap<String, usize>,
    by_pin: HashMap<(String, String), usize>,
}

impl Layout {
    /// Reads the final DEF of a flow, with the technology and cell LEFs for
    /// geometry and, if given, a SPEF file for net capacitance.
    pub fn load(def: &Path, lefs: &[&Path], spef: Option<&Path>) -> Result<Layout, String> {
        let mut lef = Lef::default();
        for p in lefs {
            lef.read(&std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?);
        }
        let text = std::fs::read_to_string(def).map_err(|e| format!("{}: {e}", def.display()))?;
        let mut l = def::read(&text, &lef)?;
        if let Some(s) = spef {
            let caps = spef::read(&std::fs::read_to_string(s).map_err(|e| format!("{}: {e}", s.display()))?);
            for n in &mut l.nets {
                n.cap = caps.get(&n.name).copied();
            }
        }
        Ok(l)
    }

    pub(crate) fn index(&mut self) {
        self.by_name = self.components.iter().enumerate().map(|(i, c)| (c.name.clone(), i)).collect();
        self.by_pin.clear();
        for (i, n) in self.nets.iter().enumerate() {
            for p in &n.pins {
                self.by_pin.insert(p.clone(), i);
            }
        }
    }

    pub fn layer_index(&self, name: &str) -> Option<u8> {
        self.layers.iter().position(|l| l.name == name).map(|i| i as u8)
    }

    pub fn component(&self, name: &str) -> Option<&Component> {
        self.by_name.get(name).map(|&i| &self.components[i])
    }

    /// The net connected to `pin` of component `comp`.
    pub fn net_at(&self, comp: &str, pin: &str) -> Option<&Net> {
        self.by_pin.get(&(comp.to_string(), pin.to_string())).map(|&i| &self.nets[i])
    }

    /// The layout name of a gate-level netlist cell (`u_core._c12_` ->
    /// `u_core/_c12_`).
    pub fn component_name(netlist_name: &str) -> String {
        netlist_name.replace('.', "/")
    }
}
