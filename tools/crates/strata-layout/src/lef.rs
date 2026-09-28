//! The parts of LEF the layout view needs: routing layers (order, default
//! width, preferred direction), via layer pairs, and cell sizes.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Layer {
    pub name: String,
    /// Default wire width (µm).
    pub width: f64,
    pub horizontal: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Lef {
    /// Routing layers, bottom first (li1, met1, ... met5 for sky130).
    pub layers: Vec<Layer>,
    /// Via name -> (lower, upper) routing layer indices.
    pub vias: HashMap<String, (u8, u8)>,
    /// Cell and site name -> (width, height) in µm.
    pub macros: HashMap<String, (f64, f64)>,
}

impl Lef {
    pub fn layer_index(&self, name: &str) -> Option<u8> {
        self.layers.iter().position(|l| l.name == name).map(|i| i as u8)
    }

    /// Adds the definitions in a LEF file (technology or cell library).
    pub fn read(&mut self, text: &str) {
        let toks: Vec<&str> = text
            .lines()
            .map(|l| l.split('#').next().unwrap_or(""))
            .flat_map(|l| l.split_whitespace())
            .collect();
        let mut i = 0;
        while i < toks.len() {
            match toks[i] {
                "PROPERTYDEFINITIONS" => {
                    // Declares properties, including "LAYER <property>" lines.
                    i = find_end(&toks, i + 1, "PROPERTYDEFINITIONS").map_or(i + 1, |e| e + 2);
                }
                "LAYER" if i + 1 < toks.len() => {
                    let name = toks[i + 1].to_string();
                    let Some(end) = find_end(&toks, i + 2, &name) else {
                        i += 2;
                        continue;
                    };
                    let body = &toks[i + 2..end];
                    let routing = body.windows(2).any(|w| w == ["TYPE", "ROUTING"]);
                    if routing {
                        let width = body.windows(2).find(|w| w[0] == "WIDTH").and_then(|w| w[1].parse().ok()).unwrap_or(0.14);
                        let horizontal = body.windows(2).any(|w| w == ["DIRECTION", "HORIZONTAL"]);
                        if self.layer_index(&name).is_none() {
                            self.layers.push(Layer { name, width, horizontal });
                        }
                    }
                    i = end + 2;
                }
                "VIA" if i + 1 < toks.len() => {
                    let name = toks[i + 1].to_string();
                    let Some(end) = find_end(&toks, i + 2, &name) else {
                        i += 2;
                        continue;
                    };
                    let used: Vec<u8> = toks[i + 2..end]
                        .windows(2)
                        .filter(|w| w[0] == "LAYER")
                        .filter_map(|w| self.layer_index(w[1].trim_end_matches(';')))
                        .collect();
                    if let (Some(lo), Some(hi)) = (used.iter().min(), used.iter().max()) {
                        self.vias.insert(name, (*lo, *hi));
                    }
                    i = end + 2;
                }
                "MACRO" | "SITE" if i + 1 < toks.len() && toks[i + 2] != ";" => {
                    let name = toks[i + 1].to_string();
                    let Some(end) = find_end(&toks, i + 2, &name) else {
                        i += 2;
                        continue;
                    };
                    if let Some(k) = toks[i + 2..end].iter().position(|t| *t == "SIZE") {
                        let b = &toks[i + 2 + k..];
                        if let (Ok(w), Ok(h)) = (b[1].parse::<f64>(), b[3].parse::<f64>()) {
                            self.macros.insert(name, (w, h));
                        }
                    }
                    i = end + 2;
                }
                _ => i += 1,
            }
        }
    }
}

/// Index of the `END <name>` that closes a block starting at `from`.
fn find_end(toks: &[&str], from: usize, name: &str) -> Option<usize> {
    (from..toks.len().saturating_sub(1)).find(|&j| toks[j] == "END" && toks[j + 1] == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_layers_vias_macros() {
        let mut l = Lef::default();
        l.read(
            "PROPERTYDEFINITIONS\n LAYER LEF58_TYPE STRING ;\nEND PROPERTYDEFINITIONS\n\
             LAYER li1\n TYPE ROUTING ;\n DIRECTION VERTICAL ;\n WIDTH 0.17 ; # c\nEND li1\n\
             LAYER mcon\n TYPE CUT ;\nEND mcon\n\
             LAYER met1\n TYPE ROUTING ;\n DIRECTION HORIZONTAL ;\n WIDTH 0.14 ;\nEND met1\n\
             VIA L1M1_PR DEFAULT\n LAYER li1 ;\n RECT -0.1 -0.1 0.1 0.1 ;\n LAYER mcon ;\n LAYER met1 ;\nEND L1M1_PR\n\
             MACRO sky130_fd_sc_hd__nand2_1\n CLASS CORE ;\n SIZE 1.380 BY 2.720 ;\n PIN A\n END A\nEND sky130_fd_sc_hd__nand2_1\n",
        );
        assert_eq!(l.layers.len(), 2);
        assert!(!l.layers[0].horizontal && l.layers[1].horizontal);
        assert_eq!(l.vias["L1M1_PR"], (0, 1));
        assert_eq!(l.macros["sky130_fd_sc_hd__nand2_1"], (1.38, 2.72));
    }
}
