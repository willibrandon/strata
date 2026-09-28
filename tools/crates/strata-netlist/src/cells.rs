//! Typed view of a Liberty library for logic simulation and analysis.

use std::collections::HashMap;

use crate::func::{Expr, FuncError, TruthTable};
use crate::liberty::{self, Group};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Input,
    Output,
    Inout,
}

/// One output of a cell, as a truth table over the cell's inputs (for
/// combinational cells) or over the single state variable (for flip-flops).
#[derive(Debug, Clone)]
pub struct OutputDef {
    pub name: String,
    pub function: String,
    pub table: TruthTable,
}

/// Edge-triggered state element (`ff` group).
#[derive(Debug, Clone)]
pub struct SeqDef {
    pub clock_pin: String,
    /// `clocked_on` is the inverted clock (falling-edge flop).
    pub clock_inverted: bool,
    /// Non-clock inputs, in `CellDef::inputs` order.
    pub data_inputs: Vec<String>,
    /// Next state over `data_inputs` followed by the current state.
    pub next_state: TruthTable,
    pub next_state_function: String,
}

#[derive(Debug, Clone)]
pub struct CellDef {
    pub name: String,
    pub area: f64,
    /// Signal inputs in library order (power pins excluded).
    pub inputs: Vec<String>,
    /// Pin capacitance of each input, in library units (pF for sky130).
    pub input_caps: Vec<f64>,
    pub outputs: Vec<OutputDef>,
    pub seq: Option<SeqDef>,
}

impl CellDef {
    pub fn is_sequential(&self) -> bool {
        self.seq.is_some()
    }

    pub fn input_index(&self, pin: &str) -> Option<usize> {
        self.inputs.iter().position(|p| p == pin)
    }

    pub fn output_index(&self, pin: &str) -> Option<usize> {
        self.outputs.iter().position(|p| p.name == pin)
    }
}

/// Cells of a library that can be simulated, plus the raw group tree for
/// other uses (timing tables for transistor-level comparisons).
pub struct CellLibrary {
    pub name: String,
    pub cells: HashMap<String, CellDef>,
    /// Cells present in the library but not usable for logic simulation, with
    /// the reason (latches, clock gates with state tables, async set/reset...).
    pub unsupported: HashMap<String, String>,
    pub raw: Group,
}

#[derive(Debug)]
pub struct LibError(pub String);

impl std::fmt::Display for LibError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LibError {}

impl From<liberty::ParseError> for LibError {
    fn from(e: liberty::ParseError) -> Self {
        LibError(e.to_string())
    }
}

impl From<FuncError> for LibError {
    fn from(e: FuncError) -> Self {
        LibError(e.to_string())
    }
}

impl CellLibrary {
    pub fn parse(src: &str) -> Result<CellLibrary, LibError> {
        let raw = liberty::parse(src)?;
        let mut cells = HashMap::new();
        let mut unsupported = HashMap::new();
        for g in raw.groups_of("cell") {
            match cell_def(g) {
                Ok(c) => {
                    cells.insert(c.name.clone(), c);
                }
                Err(e) => {
                    unsupported.insert(g.name().to_string(), e.0);
                }
            }
        }
        Ok(CellLibrary { name: raw.name().to_string(), cells, unsupported, raw })
    }

    pub fn load(path: &std::path::Path) -> Result<CellLibrary, LibError> {
        let src = std::fs::read_to_string(path)
            .map_err(|e| LibError(format!("{}: {e}", path.display())))?;
        Self::parse(&src)
    }

    pub fn raw_cell(&self, name: &str) -> Option<&Group> {
        self.raw.groups_of("cell").find(|g| g.name() == name)
    }
}

fn cell_def(g: &Group) -> Result<CellDef, LibError> {
    if g.group("latch").is_some() || g.group("statetable").is_some() {
        return Err(LibError("level-sensitive or state-table cell".into()));
    }
    let mut inputs = Vec::new();
    let mut input_caps = Vec::new();
    let mut out_pins = Vec::new();
    for p in g.groups_of("pin") {
        let dir = match p.attr("direction") {
            Some("input") => Dir::Input,
            Some("output") => Dir::Output,
            Some("inout") => Dir::Inout,
            other => return Err(LibError(format!("pin {} has direction {other:?}", p.name()))),
        };
        match dir {
            Dir::Input => {
                inputs.push(p.name().to_string());
                input_caps.push(p.attr_f64("capacitance").unwrap_or(0.0));
            }
            Dir::Output => out_pins.push(p),
            Dir::Inout => return Err(LibError("inout pin".into())),
        }
    }

    let seq = match g.group("ff") {
        None => None,
        Some(ff) => {
            if ff.attr("clear").is_some() || ff.attr("preset").is_some() {
                return Err(LibError("asynchronous clear/preset".into()));
            }
            let clk = ff.attr("clocked_on").ok_or_else(|| LibError("ff without clocked_on".into()))?;
            let (clock_pin, clock_inverted) = match Expr::parse(clk)? {
                Expr::Var(v) => (v, false),
                Expr::Not(b) => match *b {
                    Expr::Var(v) => (v, true),
                    _ => return Err(LibError(format!("complex clock \"{clk}\""))),
                },
                _ => return Err(LibError(format!("complex clock \"{clk}\""))),
            };
            let iq = ff.args.first().cloned().unwrap_or_else(|| "IQ".into());
            let next = ff.attr("next_state").ok_or_else(|| LibError("ff without next_state".into()))?;
            let data_inputs: Vec<String> = inputs.iter().filter(|i| **i != clock_pin).cloned().collect();
            let mut vars = data_inputs.clone();
            vars.push(iq);
            let next_state = Expr::parse(next)?.truth_table(&vars)?;
            Some(SeqDef {
                clock_pin,
                clock_inverted,
                data_inputs,
                next_state,
                next_state_function: next.to_string(),
            })
        }
    };

    let mut outputs = Vec::new();
    for p in out_pins {
        let f = p
            .attr("function")
            .ok_or_else(|| LibError(format!("output {} has no function", p.name())))?;
        let table = match (&seq, g.group("ff")) {
            (Some(_), Some(ff)) => {
                // Outputs of a flop are functions of the state variables.
                let iq = ff.args.first().cloned().unwrap_or_else(|| "IQ".into());
                let iqn = ff.args.get(1).cloned().unwrap_or_else(|| "IQ_N".into());
                let e = Expr::parse(f)?;
                let e = substitute_not(&e, &iqn, &iq);
                e.truth_table(&[iq])?
            }
            _ => Expr::parse(f)?.truth_table(&inputs)?,
        };
        outputs.push(OutputDef { name: p.name().to_string(), function: f.to_string(), table });
    }
    if outputs.is_empty() {
        return Err(LibError("no outputs".into()));
    }
    Ok(CellDef {
        name: g.name().to_string(),
        area: g.attr_f64("area").unwrap_or(0.0),
        inputs,
        input_caps,
        outputs,
        seq,
    })
}

/// Replaces variable `from` by `!to`.
fn substitute_not(e: &Expr, from: &str, to: &str) -> Expr {
    match e {
        Expr::Var(v) if v == from => Expr::Not(Box::new(Expr::Var(to.to_string()))),
        Expr::Var(_) | Expr::Const(_) => e.clone(),
        Expr::Not(a) => Expr::Not(Box::new(substitute_not(a, from, to))),
        Expr::And(a, b) => Expr::And(Box::new(substitute_not(a, from, to)), Box::new(substitute_not(b, from, to))),
        Expr::Or(a, b) => Expr::Or(Box::new(substitute_not(a, from, to)), Box::new(substitute_not(b, from, to))),
        Expr::Xor(a, b) => Expr::Xor(Box::new(substitute_not(a, from, to)), Box::new(substitute_not(b, from, to))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flop_and_gate() {
        let src = r#"library (t) {
          cell (nand2) { area : 3.7;
            pin (A) { direction : input; capacitance : 0.002; }
            pin (B) { direction : input; capacitance : 0.003; }
            pin (Y) { direction : output; function : "(!A) | (!B)"; } }
          cell (edff) { ff ("IQ","IQ_N") { clocked_on : "CLK"; next_state : "(D&DE) | (IQ&!DE)"; }
            pin (CLK) { direction : input; }
            pin (D) { direction : input; }
            pin (DE) { direction : input; }
            pin (Q) { direction : output; function : "IQ"; }
            pin (Q_N) { direction : output; function : "IQ_N"; } }
          cell (dff_r) { ff ("IQ","IQ_N") { clocked_on : "CLK"; next_state : "D"; clear : "!R"; }
            pin (CLK) { direction : input; } pin (D) { direction : input; } pin (R) { direction : input; }
            pin (Q) { direction : output; function : "IQ"; } }
        }"#;
        let lib = CellLibrary::parse(src).unwrap();
        let n = &lib.cells["nand2"];
        assert_eq!(n.inputs, ["A", "B"]);
        assert!(n.outputs[0].table.get(0) && !n.outputs[0].table.get(3));
        let f = &lib.cells["edff"];
        let s = f.seq.as_ref().unwrap();
        assert_eq!(s.data_inputs, ["D", "DE"]);
        // rows: D | DE<<1 | IQ<<2
        assert!(s.next_state.get(0b011)); // DE=1, D=1
        assert!(s.next_state.get(0b100)); // DE=0 keeps IQ=1
        assert!(!s.next_state.get(0b110)); // DE=1, D=0
        assert!(f.outputs[1].table.get(0) && !f.outputs[1].table.get(1)); // Q_N = !IQ
        assert!(lib.unsupported.contains_key("dff_r"));
    }
}
