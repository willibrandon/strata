//! Building ngspice decks around standard-cell instances.

use std::fmt::Write;

use crate::pdk::{Pdk, Subckt, VDD};

/// Which device models a deck loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Models {
    /// Only the devices the standard cells use (fast).
    Cells,
    /// The PDK's complete typical-corner library.
    Full,
}

/// Deck header: models, the cell netlists and the supplies (`vpwr`, `vgnd`).
pub fn header(pdk: &Pdk, title: &str, models: Models) -> String {
    let mut s = format!("* {title}\n");
    s += &match models {
        Models::Cells => pdk.models(),
        Models::Full => pdk.full_models(),
    };
    let _ = writeln!(s, ".option scale=1e-6");
    // The temperature the Liberty library was characterized at.
    let _ = writeln!(s, ".temp 25");
    let _ = writeln!(s, ".include \"{}\"", pdk.cell_spice().display());
    let _ = writeln!(s, "Vpwr vpwr 0 {VDD}\nVgnd vgnd 0 0");
    s
}

/// An instance of `cell` with its supply pins on the rails and each signal
/// pin on the node that `node` names.
pub fn instance(name: &str, cell: &Subckt, node: impl Fn(&str) -> String) -> String {
    let mut s = format!("X{name}");
    for p in &cell.ports {
        let n = match p.as_str() {
            "VPWR" | "VPB" => "vpwr".to_string(),
            "VGND" | "VNB" => "vgnd".to_string(),
            other => node(other),
        };
        s.push(' ');
        s += &n;
    }
    s.push(' ');
    s += &cell.name;
    s.push('\n');
    s
}

/// A piecewise-linear voltage source.
pub fn pwl(name: &str, node: &str, points: &[(f64, f64)]) -> String {
    let mut s = format!("V{name} {node} 0 PWL(");
    for (i, (t, v)) in points.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{t:.6e} {v}");
    }
    s += ")\n";
    s
}

/// Logic levels with transitions of `edge` seconds at the given times:
/// `levels[k]` applies from `times[k]`.
pub fn digital(times: &[f64], levels: &[bool], edge: f64) -> Vec<(f64, f64)> {
    let v = |b: bool| if b { VDD } else { 0.0 };
    let mut pts = vec![(0.0, v(levels[0]))];
    for k in 1..levels.len() {
        if levels[k] != levels[k - 1] {
            pts.push((times[k], v(levels[k - 1])));
            pts.push((times[k] + edge, v(levels[k])));
        }
    }
    pts
}
