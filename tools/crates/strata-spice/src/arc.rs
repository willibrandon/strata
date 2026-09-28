//! Transistor-level simulation of one timing arc, compared with the Liberty
//! tables for the same input transition and load.
//!
//! Combinational arcs: the other inputs are held at values under which the
//! output depends on the switching input (the arc's `when` condition when the
//! library gives one), and the input switches once with a linear ramp.
//! Flip-flop arcs: a first clock edge loads the opposite value, then the data
//! input changes and the measured clock edge loads the new value.
//!
//! Delay is measured between the input and output threshold crossings and
//! transition between the slew thresholds, as the library defines them.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;
use strata_netlist::{CellDef, CellLibrary, Expr};

use crate::deck::{self, Models};
use crate::ngspice::{self, Waves};
use crate::pdk::{Pdk, Subckt, VDD};
use crate::timing::{self, Thresholds};

#[derive(Debug, Clone, Serialize)]
pub struct ArcSpec {
    pub cell: String,
    /// Switching input (the clock for a flip-flop).
    pub input: String,
    pub output: String,
    /// Direction of the output transition.
    pub output_rise: bool,
    /// Input transition as the library defines it (ns).
    pub slew: f64,
    /// Output load (pF).
    pub load: f64,
    /// Values of the other inputs; chosen automatically when empty.
    pub side: Vec<(String, bool)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArcResult {
    pub spec: ArcSpec,
    /// Direction of the input transition that produces the output edge.
    pub input_rise: bool,
    pub side: Vec<(String, bool)>,
    /// Measured at transistor level (ns).
    pub delay: f64,
    pub transition: f64,
    /// From the Liberty tables at the same slew and load (ns).
    pub liberty_delay: Option<f64>,
    pub liberty_transition: Option<f64>,
    /// The `when` condition of the Liberty arc used, if any.
    pub liberty_when: Option<String>,
    #[serde(skip)]
    pub waves: Waves,
}

/// Values of the other inputs under which `input` controls `output`, and
/// the input edge that makes the output move in the requested direction.
fn sensitize(def: &CellDef, input: &str, output: &str, output_rise: bool, when: Option<&str>) -> Option<(Vec<(String, bool)>, bool)> {
    let k = def.input_index(input)?;
    let out = &def.outputs[def.output_index(output)?];
    let cond = when.and_then(|w| Expr::parse(w).ok());
    let n = def.inputs.len();
    for rest in 0..1usize << n {
        if (rest >> k) & 1 == 1 {
            continue;
        }
        if let Some(c) = &cond {
            let ok = c.eval(&|name| def.input_index(name).is_some_and(|j| (rest >> j) & 1 == 1));
            if !ok {
                continue;
            }
        }
        let lo = out.table.get(rest);
        let hi = out.table.get(rest | 1 << k);
        if lo != hi {
            let input_rise = hi == output_rise;
            let side = def.inputs.iter().enumerate().filter(|(j, _)| *j != k).map(|(j, p)| (p.clone(), (rest >> j) & 1 == 1)).collect();
            return Some((side, input_rise));
        }
    }
    None
}

/// Simulates one arc in `work`.
pub fn simulate(
    pdk: &Pdk,
    lib: &CellLibrary,
    subckts: &HashMap<String, Subckt>,
    spec: &ArcSpec,
    work: &Path,
    models: Models,
) -> Result<ArcResult, String> {
    let def = lib.cells.get(&spec.cell).ok_or_else(|| format!("{}: unknown cell", spec.cell))?;
    let sub = subckts.get(&spec.cell).ok_or_else(|| format!("{}: no transistor netlist", spec.cell))?;
    let th = Thresholds::of(lib);
    let arcs = timing::arcs(lib, &spec.cell, &spec.output);
    let ramp = th.ramp_for_slew(spec.slew) * 1e-9;
    let v = |b: bool| if b { VDD } else { 0.0 };
    let mut d = deck::header(pdk, &format!("strata-spice: {} {} -> {}", spec.cell, spec.input, spec.output), models);
    let node = |p: &str| format!("n_{p}");

    let (side, input_rise, t_edge, liberty_arc);
    if let Some(seq) = &def.seq {
        if spec.input != seq.clock_pin {
            return Err(format!("{}: only clock-to-output arcs are simulated for flip-flops", spec.cell));
        }
        // Data inputs that load `value` (e.g. D = value, DE = 1).
        let k = seq.data_inputs.len();
        let loads = |value: bool| (0..1usize << k).find(|&c| (0..2).all(|st| seq.next_state.get(c | st << k) == value));
        let out = &def.outputs[def.output_index(&spec.output).ok_or("unknown output")?];
        let q_after = |state: bool| out.table.get(state as usize);
        let target_state = q_after(true) == spec.output_rise;
        let first = loads(!target_state).ok_or("cannot load the opposite state")?;
        let second = loads(target_state).ok_or("cannot load the target state")?;
        // Clock edges at `gap` (loads the opposite state) and `3 gap`
        // (measured); the data inputs change halfway between them.
        let gap = (2.0 * ramp).max(1e-9);
        for (b, pin) in seq.data_inputs.iter().enumerate() {
            let pts = deck::digital(&[0.0, 2.5 * gap], &[(first >> b) & 1 == 1, (second >> b) & 1 == 1], 20e-12);
            d += &deck::pwl(&format!("in_{pin}"), &node(pin), &pts);
        }
        let (a, b) = if seq.clock_inverted { (VDD, 0.0) } else { (0.0, VDD) };
        let clk = [(0.0, a), (gap, a), (gap + ramp, b), (2.0 * gap, b), (2.0 * gap + ramp, a), (3.0 * gap, a), (3.0 * gap + ramp, b)];
        d += &deck::pwl("clk", &node(&seq.clock_pin), &clk);
        side = seq.data_inputs.iter().enumerate().map(|(b, p)| (p.clone(), (second >> b) & 1 == 1)).collect::<Vec<_>>();
        input_rise = !seq.clock_inverted;
        t_edge = 3.0 * gap;
        liberty_arc = arcs.iter().find(|a| a.related_pin == spec.input && a.timing_type.contains("edge")).cloned();
    } else {
        let chosen = if spec.side.is_empty() {
            let mut found = None;
            for a in arcs.iter().filter(|a| a.related_pin == spec.input) {
                if let Some(s) = sensitize(def, &spec.input, &spec.output, spec.output_rise, a.when.as_deref()) {
                    found = Some((s, Some(a.clone())));
                    break;
                }
            }
            match found {
                Some(f) => f,
                None => (sensitize(def, &spec.input, &spec.output, spec.output_rise, None).ok_or("the input cannot switch the output")?, None),
            }
        } else {
            let k = def.input_index(&spec.input).ok_or("unknown input")?;
            let mut rest = 0usize;
            for (p, b) in &spec.side {
                if *b {
                    rest |= 1 << def.input_index(p).ok_or("unknown side input")?;
                }
            }
            let out = &def.outputs[def.output_index(&spec.output).ok_or("unknown output")?];
            let (lo, hi) = (out.table.get(rest), out.table.get(rest | 1 << k));
            if lo == hi {
                return Err("with these side inputs the input does not switch the output".into());
            }
            let arc = arcs
                .iter()
                .find(|a| {
                    a.related_pin == spec.input
                        && a.when.as_deref().and_then(|w| Expr::parse(w).ok()).is_none_or(|c| {
                            c.eval(&|name| spec.side.iter().any(|(p, b)| p == name && *b))
                        })
                })
                .cloned();
            ((spec.side.clone(), hi == spec.output_rise), arc)
        };
        let ((s, rise), arc) = chosen;
        for (p, b) in &s {
            d += &format!("Vin_{p} {} 0 {}\n", node(p), v(*b));
        }
        d += &deck::pwl("sw", &node(&spec.input), &[(0.0, v(!rise)), (1e-9, v(!rise)), (1e-9 + ramp, v(rise))]);
        side = s;
        input_rise = rise;
        t_edge = 1e-9;
        liberty_arc = arc.or_else(|| arcs.iter().find(|a| a.related_pin == spec.input).cloned());
    }
    d += &deck::instance("dut", sub, node);
    d += &format!("Cload {} 0 {:e}\n", node(&spec.output), spec.load * 1e-12);
    let (vin, vout) = (format!("v({})", node(&spec.input)), format!("v({})", node(&spec.output)));
    // Long enough for the slowest output edge in the tables (about 20 ns
    // per pF, plus the input ramp).
    let tstop = t_edge + ramp + 2e-9 + 20.0 * spec.load * 1e-9;
    d += &format!(".save {vin} {vout}\n.tran 1p {tstop:e}\n.end\n");
    let out = ngspice::run(&d, work, true)?;
    let waves = out.waves.unwrap_or_default();
    let (Some(win), Some(wout)) = (waves.get(&vin), waves.get(&vout)) else {
        return Err("simulation produced no waveforms".into());
    };
    let after = t_edge - 0.05e-9;
    let at = |w: &[f64], frac: f64, rise: bool| crossing(&waves.time, w, frac * VDD, rise, after);
    let t_in = at(win, th.input, input_rise).ok_or("the input did not switch")?;
    let t_out = at(wout, th.output, spec.output_rise).ok_or("the output did not switch")?;
    let (a, b) = if spec.output_rise { (th.slew_lower, th.slew_upper) } else { (th.slew_upper, th.slew_lower) };
    let t_a = at(wout, a, spec.output_rise).ok_or("the output did not complete its transition")?;
    let t_b = at(wout, b, spec.output_rise).ok_or("the output did not complete its transition")?;
    let delay = (t_out - t_in) * 1e9;
    let transition = (t_b - t_a) * 1e9 / th.slew_derate;
    let (ld, lt) = match &liberty_arc {
        Some(a) => {
            let (dt, tt) = a.tables(spec.output_rise);
            (dt.map(|t| t.lookup(spec.slew, spec.load)), tt.map(|t| t.lookup(spec.slew, spec.load)))
        }
        None => (None, None),
    };
    Ok(ArcResult {
        spec: spec.clone(),
        input_rise,
        side,
        delay,
        transition,
        liberty_delay: ld,
        liberty_transition: lt,
        liberty_when: liberty_arc.and_then(|a| a.when),
        waves,
    })
}

/// First time after `after` at which `v` crosses `level` in the given
/// direction, interpolated between samples.
pub fn crossing(time: &[f64], v: &[f64], level: f64, rising: bool, after: f64) -> Option<f64> {
    for k in 1..time.len() {
        if time[k] < after {
            continue;
        }
        let (a, b) = (v[k - 1], v[k]);
        let crosses = if rising { a < level && b >= level } else { a > level && b <= level };
        if crosses {
            let f = (level - a) / (b - a);
            return Some(time[k - 1] + f * (time[k] - time[k - 1]));
        }
    }
    None
}
