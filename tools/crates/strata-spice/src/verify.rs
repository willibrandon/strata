//! Checks the Boolean behavior of cells at transistor level against the
//! Liberty functions that the gate-level simulator evaluates.
//!
//! Combinational cells get every input combination, held long enough to
//! settle; the output voltage is sampled at the end of each interval.
//! Flip-flops get a pseudo-random sequence of data inputs, one clock pulse
//! per step, and the output is sampled after each rising edge and compared
//! with the state predicted by the Liberty `next_state` function. A voltage
//! within 20% of a rail is a logic value; anything else is a failure.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;
use strata_netlist::{CellDef, CellLibrary};

use crate::deck::{self, Models};
use crate::ngspice;
use crate::pdk::{Pdk, Subckt, VDD};

const ROW: f64 = 1e-9; // combinational: time per input vector
const STEP: f64 = 2e-9; // sequential: time per clock step
const EDGE: f64 = 20e-12;
const LOAD: &str = "2f";

#[derive(Debug, Clone, Serialize)]
pub struct Mismatch {
    pub vector: usize,
    /// Input values, `PIN=v` in library order.
    pub inputs: String,
    pub output: String,
    pub expected: bool,
    pub volts: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CellCheck {
    pub cell: String,
    pub kind: &'static str,
    pub transistors: usize,
    pub vectors: usize,
    /// Output samples compared.
    pub checks: usize,
    pub mismatches: Vec<Mismatch>,
    /// Lowest sampled voltage of an output at logic 1 and highest at logic 0.
    pub min_high: f64,
    pub max_low: f64,
}

struct Plan {
    cell: String,
    kind: &'static str,
    vectors: usize,
    /// (measure name, output pin, vector, expected, input description).
    samples: Vec<(String, String, usize, bool, String)>,
}

fn level(v: f64) -> Option<bool> {
    if v >= 0.8 * VDD {
        Some(true)
    } else if v <= 0.2 * VDD {
        Some(false)
    } else {
        None
    }
}

fn describe(names: &[String], bits: &[bool]) -> String {
    names.iter().zip(bits).map(|(n, b)| format!("{n}={}", *b as u8)).collect::<Vec<_>>().join(" ")
}

/// Adds the stimulus and measurements for one combinational cell.
fn combinational(i: usize, def: &CellDef, sub: &Subckt, d: &mut String) -> Plan {
    let n = def.inputs.len();
    let rows = 1usize << n;
    let node = |p: &str| format!("c{i}_{p}");
    for (k, pin) in def.inputs.iter().enumerate() {
        let times: Vec<f64> = (0..rows).map(|r| r as f64 * ROW).collect();
        let levels: Vec<bool> = (0..rows).map(|r| (r >> k) & 1 == 1).collect();
        *d += &deck::pwl(&format!("c{i}_{pin}"), &node(pin), &deck::digital(&times, &levels, EDGE));
    }
    *d += &deck::instance(&format!("c{i}"), sub, node);
    let mut samples = Vec::new();
    for (o, out) in def.outputs.iter().enumerate() {
        *d += &format!("Cc{i}_{} {} 0 {LOAD}\n", out.name, node(&out.name));
        for r in 0..rows {
            let m = format!("m{i}_{o}_{r}");
            *d += &format!(".meas tran {m} FIND v({}) AT={:.6e}\n", node(&out.name), (r + 1) as f64 * ROW - 0.05e-9);
            let bits: Vec<bool> = (0..n).map(|k| (r >> k) & 1 == 1).collect();
            samples.push((m, out.name.clone(), r, out.table.get(r), describe(&def.inputs, &bits)));
        }
    }
    Plan { cell: def.name.clone(), kind: if n == 0 { "constant" } else { "combinational" }, vectors: rows, samples }
}

/// Adds the stimulus and measurements for one edge-triggered flip-flop.
fn sequential(i: usize, def: &CellDef, sub: &Subckt, d: &mut String) -> Plan {
    let seq = def.seq.as_ref().expect("sequential cell");
    let k = seq.data_inputs.len();
    let combos = 1usize << k;
    // A fixed pseudo-random sequence that visits every combination from
    // several different states.
    let mut x: u32 = 0x2545_f491;
    let steps: Vec<usize> = (0..combos * 8)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as usize) % combos
        })
        .collect();
    let node = |p: &str| format!("c{i}_{p}");
    for (b, pin) in seq.data_inputs.iter().enumerate() {
        let times: Vec<f64> = (0..steps.len()).map(|s| s as f64 * STEP + 0.1e-9).collect();
        let levels: Vec<bool> = steps.iter().map(|c| (c >> b) & 1 == 1).collect();
        *d += &deck::pwl(&format!("c{i}_{pin}"), &node(pin), &deck::digital(&times, &levels, EDGE));
    }
    let (rise, fall) = if seq.clock_inverted { (1.5e-9, 1.0e-9) } else { (1.0e-9, 1.5e-9) };
    let mut clk = vec![(0.0, if seq.clock_inverted { VDD } else { 0.0 })];
    for s in 0..steps.len() {
        let t0 = s as f64 * STEP;
        let (first, second) = if seq.clock_inverted { (fall, rise) } else { (rise, fall) };
        let (lo, hi) = if seq.clock_inverted { (VDD, 0.0) } else { (0.0, VDD) };
        clk.push((t0 + first, lo));
        clk.push((t0 + first + EDGE, hi));
        clk.push((t0 + second, hi));
        clk.push((t0 + second + EDGE, lo));
    }
    *d += &deck::pwl(&format!("c{i}_{}", seq.clock_pin), &node(&seq.clock_pin), &clk);
    *d += &deck::instance(&format!("c{i}"), sub, node);
    let mut samples = Vec::new();
    for out in &def.outputs {
        *d += &format!("Cc{i}_{} {} 0 {LOAD}\n", out.name, node(&out.name));
    }
    let mut state: Option<bool> = None;
    for (s, &c) in steps.iter().enumerate() {
        let next = |st: bool| seq.next_state.get(c | (st as usize) << k);
        state = match state {
            Some(st) => Some(next(st)),
            None if next(false) == next(true) => Some(next(false)),
            None => None,
        };
        let Some(st) = state else { continue };
        let bits: Vec<bool> = (0..k).map(|b| (c >> b) & 1 == 1).collect();
        for (o, out) in def.outputs.iter().enumerate() {
            let m = format!("m{i}_{o}_{s}");
            *d += &format!(".meas tran {m} FIND v({}) AT={:.6e}\n", node(&out.name), s as f64 * STEP + 1.9e-9);
            samples.push((m, out.name.clone(), s, out.table.get(st as usize), describe(&seq.data_inputs, &bits)));
        }
    }
    Plan { cell: def.name.clone(), kind: "flip-flop", vectors: steps.len(), samples }
}

/// Checks `cells` (library names) in one ngspice run in `work`.
pub fn check_cells(
    pdk: &Pdk,
    lib: &CellLibrary,
    subckts: &HashMap<String, Subckt>,
    cells: &[String],
    work: &Path,
    models: Models,
) -> Result<Vec<CellCheck>, String> {
    let mut d = deck::header(pdk, "strata-spice: logic function of standard cells", models);
    let mut plans = Vec::new();
    let mut tstop: f64 = 0.0;
    for (i, name) in cells.iter().enumerate() {
        let def = lib.cells.get(name).ok_or_else(|| format!("{name}: not a simulatable Liberty cell"))?;
        let sub = subckts.get(name).ok_or_else(|| format!("{name}: no transistor netlist in the PDK"))?;
        let plan = if def.is_sequential() { sequential(i, def, sub, &mut d) } else { combinational(i, def, sub, &mut d) };
        let period = if def.is_sequential() { STEP } else { ROW };
        tstop = tstop.max(plan.vectors as f64 * period);
        plans.push(plan);
    }
    d += &format!(".tran 10p {tstop:.6e}\n.end\n");
    let out = ngspice::run(&d, work, false)?;
    let mut results = Vec::new();
    for (plan, name) in plans.iter().zip(cells) {
        let sub = &subckts[name];
        let mut r = CellCheck {
            cell: plan.cell.clone(),
            kind: plan.kind,
            transistors: sub.mosfets.len(),
            vectors: plan.vectors,
            checks: plan.samples.len(),
            mismatches: Vec::new(),
            min_high: VDD,
            max_low: 0.0,
        };
        for (m, pin, vector, expected, inputs) in &plan.samples {
            let v = out.measures.get(m).copied().ok_or_else(|| format!("measurement {m} missing"))?;
            if *expected {
                r.min_high = r.min_high.min(v);
            } else {
                r.max_low = r.max_low.max(v);
            }
            if level(v) != Some(*expected) {
                r.mismatches.push(Mismatch {
                    vector: *vector,
                    inputs: inputs.clone(),
                    output: pin.clone(),
                    expected: *expected,
                    volts: v,
                });
            }
        }
        results.push(r);
    }
    Ok(results)
}
