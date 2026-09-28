//! Liberty timing (NLDM tables): propagation delay and output transition of
//! each timing arc as a function of input transition and output load.

use serde::Serialize;
use strata_netlist::liberty::Group;
use strata_netlist::CellLibrary;

/// Measurement conventions of the library (fractions of the supply).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Thresholds {
    pub input: f64,
    pub output: f64,
    pub slew_lower: f64,
    pub slew_upper: f64,
    pub slew_derate: f64,
}

impl Thresholds {
    pub fn of(lib: &CellLibrary) -> Thresholds {
        let a = |n: &str, d: f64| lib.raw.attr_f64(n).unwrap_or(d);
        Thresholds {
            input: a("input_threshold_pct_rise", 50.0) / 100.0,
            output: a("output_threshold_pct_rise", 50.0) / 100.0,
            slew_lower: a("slew_lower_threshold_pct_rise", 20.0) / 100.0,
            slew_upper: a("slew_upper_threshold_pct_rise", 80.0) / 100.0,
            slew_derate: a("slew_derate_from_library", 1.0),
        }
    }

    /// Duration of a full-swing linear ramp whose library transition time
    /// (measured between the slew thresholds, then derated) is `slew`.
    pub fn ramp_for_slew(&self, slew: f64) -> f64 {
        slew * self.slew_derate / (self.slew_upper - self.slew_lower)
    }
}

/// A lookup table over input transition (ns) and output load (pF).
#[derive(Debug, Clone, Serialize)]
pub struct Table {
    pub transition: Vec<f64>,
    pub load: Vec<f64>,
    /// `values[i][j]` for `transition[i]`, `load[j]`.
    pub values: Vec<Vec<f64>>,
}

fn numbers(v: &[String]) -> Vec<f64> {
    v.iter().flat_map(|s| s.split(',')).filter_map(|x| x.trim().trim_matches('"').trim().parse().ok()).collect()
}

impl Table {
    fn from_group(g: &Group, lib: &Group) -> Option<Table> {
        let i1 = numbers(g.complex("index_1")?);
        let i2 = g.complex("index_2").map(numbers).unwrap_or_default();
        let rows: Vec<Vec<f64>> = g.complex("values")?.iter().map(|r| numbers(std::slice::from_ref(r))).collect();
        // Which index is which comes from the table template.
        let template = lib.groups_of("lu_table_template").find(|t| g.args.first().is_some_and(|a| a == t.name()));
        let var1 = template.and_then(|t| t.attr("variable_1")).unwrap_or("input_net_transition");
        if i2.is_empty() {
            let values = rows.concat();
            return Some(if var1.contains("capacitance") {
                Table { transition: vec![0.0], load: i1, values: vec![values] }
            } else {
                Table { transition: i1, load: vec![0.0], values: values.into_iter().map(|v| vec![v]).collect() }
            });
        }
        if var1.contains("capacitance") {
            // Transpose to (transition, load).
            let values = (0..i2.len()).map(|j| (0..i1.len()).map(|i| rows[i][j]).collect()).collect();
            Some(Table { transition: i2, load: i1, values })
        } else {
            Some(Table { transition: i1, load: i2, values: rows })
        }
    }

    /// Bilinear interpolation, extrapolating linearly outside the table.
    pub fn lookup(&self, transition: f64, load: f64) -> f64 {
        fn seg(xs: &[f64], x: f64) -> (usize, f64) {
            if xs.len() < 2 {
                return (0, 0.0);
            }
            let mut i = 0;
            while i + 2 < xs.len() && x > xs[i + 1] {
                i += 1;
            }
            (i, (x - xs[i]) / (xs[i + 1] - xs[i]))
        }
        let (i, t) = seg(&self.transition, transition);
        let (j, u) = seg(&self.load, load);
        let v = |a: usize, b: usize| self.values[a.min(self.values.len() - 1)][b.min(self.values[0].len() - 1)];
        let i1 = if self.transition.len() > 1 { i + 1 } else { i };
        let j1 = if self.load.len() > 1 { j + 1 } else { j };
        v(i, j) * (1.0 - t) * (1.0 - u) + v(i1, j) * t * (1.0 - u) + v(i, j1) * (1.0 - t) * u + v(i1, j1) * t * u
    }
}

/// One timing arc of an output pin.
#[derive(Debug, Clone, Serialize)]
pub struct Arc {
    pub related_pin: String,
    /// `combinational`, `rising_edge`, ...
    pub timing_type: String,
    pub timing_sense: String,
    /// Condition on other inputs under which the tables apply.
    pub when: Option<String>,
    pub cell_rise: Option<Table>,
    pub cell_fall: Option<Table>,
    pub rise_transition: Option<Table>,
    pub fall_transition: Option<Table>,
}

impl Arc {
    /// (delay, transition) tables for an output edge.
    pub fn tables(&self, output_rise: bool) -> (Option<&Table>, Option<&Table>) {
        if output_rise {
            (self.cell_rise.as_ref(), self.rise_transition.as_ref())
        } else {
            (self.cell_fall.as_ref(), self.fall_transition.as_ref())
        }
    }
}

/// Timing arcs ending at `output` of `cell`.
pub fn arcs(lib: &CellLibrary, cell: &str, output: &str) -> Vec<Arc> {
    let Some(c) = lib.raw_cell(cell) else { return Vec::new() };
    let Some(pin) = c.groups_of("pin").find(|p| p.name() == output) else { return Vec::new() };
    let table = |t: &Group, kind: &str| t.group(kind).and_then(|g| Table::from_group(g, &lib.raw));
    pin.groups_of("timing")
        .map(|t| Arc {
            related_pin: t.attr("related_pin").unwrap_or_default().trim_matches('"').to_string(),
            timing_type: t.attr("timing_type").unwrap_or("combinational").trim_matches('"').to_string(),
            timing_sense: t.attr("timing_sense").unwrap_or_default().trim_matches('"').to_string(),
            when: t.attr("when").map(|w| w.trim_matches('"').to_string()),
            cell_rise: table(t, "cell_rise"),
            cell_fall: table(t, "cell_fall"),
            rise_transition: table(t, "rise_transition"),
            fall_transition: table(t, "fall_transition"),
        })
        .collect()
}

/// Input pin capacitance (pF) of `pin` of `cell`.
pub fn pin_cap(lib: &CellLibrary, cell: &str, pin: &str) -> Option<f64> {
    let c = lib.cells.get(cell)?;
    if let Some(i) = c.input_index(pin) {
        return c.input_caps.get(i).copied();
    }
    lib.raw_cell(cell)?.groups_of("pin").find(|p| p.name() == pin)?.attr_f64("capacitance")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_and_extrapolates() {
        let t = Table { transition: vec![0.0, 1.0], load: vec![0.0, 2.0], values: vec![vec![0.0, 2.0], vec![1.0, 3.0]] };
        assert!((t.lookup(0.5, 1.0) - 1.5).abs() < 1e-12);
        assert!((t.lookup(2.0, 0.0) - 2.0).abs() < 1e-12);
        assert!((t.lookup(0.0, 4.0) - 4.0).abs() < 1e-12);
    }

    #[test]
    fn ramp() {
        let th = Thresholds { input: 0.5, output: 0.5, slew_lower: 0.2, slew_upper: 0.8, slew_derate: 1.0 };
        assert!((th.ramp_for_slew(0.06) - 0.1).abs() < 1e-12);
    }
}
