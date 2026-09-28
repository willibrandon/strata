//! The sky130 PDK files used for transistor-level analysis, and the
//! transistor netlists of the standard cells.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;

/// Supply voltage of the typical corner (V).
pub const VDD: f64 = 1.8;

#[derive(Debug, Clone)]
pub struct Pdk {
    /// The `sky130A` directory.
    pub root: PathBuf,
}

impl Pdk {
    /// `$PDK_ROOT/sky130A`, with `PDK_ROOT` defaulting to `~/tools/pdk`
    /// (docs/setup.md).
    pub fn locate() -> Result<Pdk, String> {
        let base = std::env::var_os("PDK_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("tools/pdk"));
        let pdk = Pdk { root: base.join("sky130A") };
        if !pdk.cell_spice().exists() {
            return Err(format!("sky130A standard-cell SPICE netlists not found under {}", base.display()));
        }
        Ok(pdk)
    }

    /// Transistor netlists of every sky130_fd_sc_hd cell.
    pub fn cell_spice(&self) -> PathBuf {
        self.root.join("libs.ref/sky130_fd_sc_hd/spice/sky130_fd_sc_hd.spice")
    }

    /// The Liberty library the gate-level model uses (typical corner).
    pub fn liberty(&self) -> PathBuf {
        self.root.join("libs.ref/sky130_fd_sc_hd/lib/sky130_fd_sc_hd__tt_025C_1v80.lib")
    }

    /// Device models for the typical corner, limited to the devices that the
    /// sky130_fd_sc_hd cells use: `nfet_01v8` (whose model file also defines
    /// `special_nfet_01v8`, used in flip-flops and multiplexers),
    /// `pfet_01v8_hvt`, and the resistor models of the tie cell. The PDK's
    /// `.lib sky130.lib.spice tt` section loads every device family and takes
    /// about 50 times longer; both give identical results (see the tests).
    pub fn models(&self) -> String {
        let ng = self.root.join("libs.tech/ngspice");
        let pr = self.root.join("libs.ref/sky130_fd_pr/spice");
        let mut s = String::from(".param mc_mm_switch=0 mc_pr_switch=0\n");
        s += &format!(".include \"{}\"\n", ng.join("all.spice").display());
        for f in [
            "sky130_fd_pr__nfet_01v8__tt.pm3.spice",
            "sky130_fd_pr__nfet_01v8__mismatch.corner.spice",
            "sky130_fd_pr__pfet_01v8_hvt__tt.pm3.spice",
            "sky130_fd_pr__pfet_01v8_hvt__mismatch.corner.spice",
        ] {
            s += &format!(".include \"{}\"\n", pr.join(f).display());
        }
        s += &format!(".include \"{}\"\n", ng.join("corners/tt/specialized_cells.spice").display());
        s += &format!(".include \"{}\"\n", ng.join("r+c/res_typical__cap_typical.spice").display());
        s += &format!(".include \"{}\"\n", ng.join("r+c/res_typical__cap_typical__lin.spice").display());
        s
    }

    /// The complete typical-corner model library of the PDK.
    pub fn full_models(&self) -> String {
        format!(".lib \"{}\" tt\n", self.root.join("libs.tech/ngspice/sky130.lib.spice").display())
    }

    /// Transistor netlists of all cells in the library.
    pub fn load_cells(&self) -> Result<HashMap<String, Subckt>, String> {
        let text = std::fs::read_to_string(self.cell_spice()).map_err(|e| e.to_string())?;
        Ok(parse_library(&text))
    }
}

/// One MOS transistor of a cell.
#[derive(Debug, Clone, Serialize)]
pub struct Mosfet {
    pub name: String,
    pub drain: String,
    pub gate: String,
    pub source: String,
    pub body: String,
    pub model: String,
    /// Channel width and length (µm).
    pub w: f64,
    pub l: f64,
}

impl Mosfet {
    pub fn is_pmos(&self) -> bool {
        self.model.contains("pfet")
    }
}

/// A cell's transistor netlist.
#[derive(Debug, Clone, Serialize)]
pub struct Subckt {
    pub name: String,
    pub ports: Vec<String>,
    pub mosfets: Vec<Mosfet>,
    /// Other elements (the tie cell's resistors), as written in the netlist.
    pub other: Vec<String>,
}

impl Subckt {
    /// Nets internal to the cell (not ports).
    pub fn internal_nets(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for m in &self.mosfets {
            for n in [&m.drain, &m.source, &m.gate] {
                if !self.ports.contains(n) && !v.contains(n) {
                    v.push(n.clone());
                }
            }
        }
        v
    }
}

/// Parses the `.subckt` definitions of a SPICE library. Device dimensions
/// are in the PDK's scaled units (`.option scale=1e-6`), i.e. micrometres.
pub fn parse_library(text: &str) -> HashMap<String, Subckt> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        let l = raw.trim();
        if let Some(rest) = l.strip_prefix('+') {
            if let Some(last) = lines.last_mut() {
                last.push(' ');
                last.push_str(rest.trim());
            }
            continue;
        }
        if l.is_empty() || l.starts_with('*') {
            continue;
        }
        lines.push(l.to_string());
    }
    let mut out = HashMap::new();
    let mut cur: Option<Subckt> = None;
    for l in lines {
        let toks: Vec<&str> = l.split_whitespace().collect();
        let head = toks[0].to_ascii_lowercase();
        if head == ".subckt" && toks.len() > 1 {
            cur = Some(Subckt {
                name: toks[1].to_string(),
                ports: toks[2..].iter().filter(|t| !t.contains('=')).map(|s| s.to_string()).collect(),
                mosfets: Vec::new(),
                other: Vec::new(),
            });
        } else if head == ".ends" {
            if let Some(s) = cur.take() {
                out.insert(s.name.clone(), s);
            }
        } else if let Some(s) = cur.as_mut() {
            let pos: Vec<&str> = toks.iter().filter(|t| !t.contains('=')).copied().collect();
            let param = |k: &str| {
                toks.iter().find_map(|t| {
                    t.split_once('=').filter(|(n, _)| n.eq_ignore_ascii_case(k)).and_then(|(_, v)| spice_number(v))
                })
            };
            let fet = pos.len() >= 6 && pos[5].contains("fet") && (head.starts_with('x') || head.starts_with('m'));
            if fet {
                s.mosfets.push(Mosfet {
                    name: pos[0].to_string(),
                    drain: pos[1].to_string(),
                    gate: pos[2].to_string(),
                    source: pos[3].to_string(),
                    body: pos[4].to_string(),
                    model: pos[5].to_string(),
                    w: param("w").unwrap_or(0.0),
                    l: param("l").unwrap_or(0.0),
                });
            } else {
                s.other.push(l.clone());
            }
        }
    }
    out
}

/// A SPICE number with an optional scale suffix: `1e+06u`, `150n`, `2meg`.
pub fn spice_number(s: &str) -> Option<f64> {
    let s = s.trim().to_ascii_lowercase();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let exp_sign = (c == b'+' || c == b'-') && (i == 0 || b[i - 1] == b'e');
        let exp = c == b'e' && i + 1 < b.len() && (b[i + 1].is_ascii_digit() || b[i + 1] == b'+' || b[i + 1] == b'-');
        if c.is_ascii_digit() || c == b'.' || exp_sign || exp {
            i += 1;
        } else {
            break;
        }
    }
    let v: f64 = s[..i].parse().ok()?;
    let rest = &s[i..];
    let mult = if rest.starts_with("meg") {
        1e6
    } else {
        match rest.chars().next() {
            Some('f') => 1e-15,
            Some('p') => 1e-12,
            Some('n') => 1e-9,
            Some('u') => 1e-6,
            Some('m') => 1e-3,
            Some('k') => 1e3,
            Some('g') => 1e9,
            Some('t') => 1e12,
            _ => 1.0,
        }
    };
    Some(v * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(spice_number("1e+06u"), Some(1.0));
        assert!((spice_number("150000u").unwrap() - 0.15).abs() < 1e-12);
        assert_eq!(spice_number("2meg"), Some(2e6));
        assert_eq!(spice_number("0.48"), Some(0.48));
        assert!((spice_number("5f").unwrap() - 5e-15).abs() < 1e-27);
    }

    #[test]
    fn parses_a_cell() {
        let lib = parse_library(
            ".subckt sky130_fd_sc_hd__nand2_1 A B VGND VNB VPB VPWR Y\n\
             X0 Y A VPWR VPB sky130_fd_pr__pfet_01v8_hvt w=1e+06u l=150000u\n\
             X1 VPWR B Y VPB sky130_fd_pr__pfet_01v8_hvt w=1e+06u l=150000u\n\
             X2 VGND B a_113_47# VNB sky130_fd_pr__nfet_01v8 w=650000u\n\
             + l=150000u\n\
             X3 a_113_47# A Y VNB sky130_fd_pr__nfet_01v8 w=650000u l=150000u\n\
             .ends\n",
        );
        let c = &lib["sky130_fd_sc_hd__nand2_1"];
        assert_eq!(c.ports, ["A", "B", "VGND", "VNB", "VPB", "VPWR", "Y"]);
        assert_eq!(c.mosfets.len(), 4);
        assert!(c.mosfets[0].is_pmos() && !c.mosfets[2].is_pmos());
        assert!((c.mosfets[2].w - 0.65).abs() < 1e-9 && (c.mosfets[2].l - 0.15).abs() < 1e-9);
        assert_eq!(c.internal_nets(), ["a_113_47#"]);
    }
}
