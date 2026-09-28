//! strata-spice: transistor-level analysis of the cells in the Strata netlist.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;

use serde::Serialize;
use strata_spice::arc::{ArcResult, ArcSpec};
use strata_spice::{timing, Analyzer, Models};

const USAGE: &str = "usage:
  strata-spice verify [--netlist <json>] [--models cells|full] [--json <file>]
      logic function of every cell type in the netlist, at transistor level,
      against the Liberty functions the gate simulator uses
  strata-spice characterize [--netlist <json>] [--cells a,b,...] [-j <n>] [--json <file>]
      delay and transition of every timing arc of the most used cells on a
      grid of input transitions and loads, against the Liberty tables
  strata-spice arc <cell> <input> <output> rise|fall [--slew <ns>] [--load <pF>]
      [--side PIN=0|1,...] [--csv <file>]
      one arc, with its waveforms
  strata-spice models [--cells a,b,...]
      checks that the reduced device model set gives the same results as the
      PDK's complete typical-corner library

Cell names may omit the sky130_fd_sc_hd__ prefix. Work files go to build/spice.";

const PREFIX: &str = "sky130_fd_sc_hd__";

fn full_name(c: &str) -> String {
    if c.starts_with(PREFIX) {
        c.to_string()
    } else {
        format!("{PREFIX}{c}")
    }
}

/// Cell types used by a yosys JSON netlist, most used first.
fn netlist_cells(path: &Path) -> Result<Vec<(String, usize)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for m in v["modules"].as_object().ok_or("no modules")?.values() {
        if let Some(cells) = m["cells"].as_object() {
            for c in cells.values() {
                if let Some(t) = c["type"].as_str().filter(|t| t.starts_with(PREFIX)) {
                    *counts.entry(t.to_string()).or_default() += 1;
                }
            }
        }
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(v)
}

struct Opts {
    positional: Vec<String>,
    flags: BTreeMap<String, String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { positional: Vec::new(), flags: BTreeMap::new() };
    let mut i = 0;
    while i < args.len() {
        if let Some(f) = args[i].strip_prefix('-') {
            let f = f.trim_start_matches('-').to_string();
            let v = args.get(i + 1).ok_or_else(|| format!("--{f} needs a value"))?.clone();
            o.flags.insert(f, v);
            i += 2;
        } else {
            o.positional.push(args[i].clone());
            i += 1;
        }
    }
    Ok(o)
}

fn write_json<T: Serialize>(path: &str, v: &T) -> Result<(), String> {
    if let Some(dir) = Path::new(path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, serde_json::to_string_pretty(v).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())
}

fn work() -> PathBuf {
    PathBuf::from("build/spice")
}

fn verify(a: &Analyzer, o: &Opts) -> Result<bool, String> {
    let netlist = PathBuf::from(o.flags.get("netlist").map(String::as_str).unwrap_or("build/gate/strata_soc.json"));
    let used = netlist_cells(&netlist)?;
    let models = match o.flags.get("models").map(String::as_str) {
        None | Some("cells") => Models::Cells,
        Some("full") => Models::Full,
        Some(m) => return Err(format!("unknown model set {m}")),
    };
    let names: Vec<String> = used.iter().map(|(n, _)| n.clone()).collect();
    let t0 = std::time::Instant::now();
    let results = a.verify(&names, &work().join("verify"), models)?;
    let mut ok = true;
    println!("{:<34} {:>6} {:>12} {:>7} {:>7} {:>9} {:>9}", "cell", "uses", "kind", "fets", "checks", "min high", "max low");
    for (r, (_, uses)) in results.iter().zip(&used) {
        println!(
            "{:<34} {:>6} {:>12} {:>7} {:>7} {:>8.3}V {:>8.3}V{}",
            r.cell.trim_start_matches(PREFIX),
            uses,
            r.kind,
            r.transistors,
            r.checks,
            r.min_high,
            r.max_low,
            if r.mismatches.is_empty() { String::new() } else { format!("  {} MISMATCHES", r.mismatches.len()) }
        );
        for m in r.mismatches.iter().take(4) {
            println!("    vector {} ({}): {} expected {} measured {:.3} V", m.vector, m.inputs, m.output, m.expected as u8, m.volts);
        }
        ok &= r.mismatches.is_empty();
    }
    let checks: usize = results.iter().map(|r| r.checks).sum();
    let bad: usize = results.iter().map(|r| r.mismatches.len()).sum();
    println!("{} cell types, {checks} output samples, {bad} mismatches ({:.1} s)", results.len(), t0.elapsed().as_secs_f64());
    if let Some(p) = o.flags.get("json") {
        #[derive(Serialize)]
        struct Out {
            category: &'static str,
            note: &'static str,
            netlist: String,
            models: &'static str,
            cell_types: usize,
            output_samples: usize,
            mismatches: usize,
            cells: Vec<serde_json::Value>,
        }
        let cells = results
            .iter()
            .zip(&used)
            .map(|(r, (_, uses))| {
                let mut v = serde_json::to_value(r).unwrap();
                v["uses"] = (*uses).into();
                v
            })
            .collect();
        write_json(
            p,
            &Out {
                category: "simulated",
                note: "ngspice transient simulation of the PDK transistor netlists, typical corner; logic levels within 20% of a rail",
                netlist: netlist.display().to_string(),
                models: if models == Models::Cells { "cells" } else { "full" },
                cell_types: results.len(),
                output_samples: checks,
                mismatches: bad,
                cells,
            },
        )?;
    }
    Ok(ok)
}

/// Every arc of `cell`: (input, output, output edge).
fn arcs_of(a: &Analyzer, cell: &str) -> Vec<(String, String, bool)> {
    let Some(def) = a.lib.cells.get(cell) else { return Vec::new() };
    let mut out = Vec::new();
    for o in &def.outputs {
        let mut inputs: Vec<String> = timing::arcs(&a.lib, cell, &o.name)
            .iter()
            .filter(|t| t.timing_type == "combinational" || t.timing_type.contains("edge"))
            .map(|t| t.related_pin.clone())
            .collect();
        inputs.dedup();
        inputs.sort();
        inputs.dedup();
        for i in inputs {
            for rise in [true, false] {
                out.push((i.clone(), o.name.clone(), rise));
            }
        }
    }
    out
}

#[derive(Serialize)]
struct Sample {
    cell: String,
    input: String,
    output: String,
    output_rise: bool,
    slew: f64,
    load: f64,
    side: Vec<(String, bool)>,
    delay: f64,
    liberty_delay: Option<f64>,
    transition: f64,
    liberty_transition: Option<f64>,
}

fn pct(m: f64, l: Option<f64>) -> Option<f64> {
    l.filter(|l| *l > 0.0).map(|l| 100.0 * (m - l) / l)
}

fn characterize(a: &Analyzer, o: &Opts) -> Result<bool, String> {
    let netlist = PathBuf::from(o.flags.get("netlist").map(String::as_str).unwrap_or("build/gate/strata_soc.json"));
    let cells: Vec<String> = match o.flags.get("cells") {
        Some(c) => c.split(',').map(full_name).collect(),
        None => {
            // The eight most used combinational cells and every flip-flop.
            let used = netlist_cells(&netlist)?;
            let mut v: Vec<String> = used
                .iter()
                .filter(|(n, _)| a.lib.cells.get(n).is_some_and(|d| !d.is_sequential() && !d.inputs.is_empty()))
                .take(8)
                .map(|(n, _)| n.clone())
                .collect();
            v.extend(used.iter().filter(|(n, _)| a.lib.cells.get(n).is_some_and(|d| d.is_sequential())).map(|(n, _)| n.clone()));
            v
        }
    };
    // Grid: the 2nd, 4th and 6th index points of the library's 7x7 tables.
    let slews = [0.0230506, 0.122474, 0.650743];
    let loads = [0.00131655, 0.00912787, 0.0632852];
    let mut jobs = Vec::new();
    for c in &cells {
        for (i, out, rise) in arcs_of(a, c) {
            for s in slews {
                for l in loads {
                    jobs.push(ArcSpec { cell: c.clone(), input: i.clone(), output: out.clone(), output_rise: rise, slew: s, load: l, side: vec![] });
                }
            }
        }
    }
    let threads: usize = o.flags.get("j").and_then(|j| j.parse().ok()).unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    println!("{} simulations of {} cells on {threads} threads", jobs.len(), cells.len());
    let next = Mutex::new(0usize);
    let results: Mutex<Vec<(usize, Result<ArcResult, String>)>> = Mutex::new(Vec::new());
    let t0 = std::time::Instant::now();
    std::thread::scope(|s| {
        for t in 0..threads {
            let (next, results, jobs) = (&next, &results, &jobs);
            s.spawn(move || loop {
                let k = {
                    let mut n = next.lock().unwrap();
                    *n += 1;
                    *n - 1
                };
                if k >= jobs.len() {
                    break;
                }
                let r = a.arc(&jobs[k], &work().join(format!("arc{t}")));
                results.lock().unwrap().push((k, r));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| r.0);
    let mut samples = Vec::new();
    let mut errors = Vec::new();
    for (k, r) in results {
        match r {
            Ok(r) => samples.push(Sample {
                cell: r.spec.cell.clone(),
                input: r.spec.input.clone(),
                output: r.spec.output.clone(),
                output_rise: r.spec.output_rise,
                slew: r.spec.slew,
                load: r.spec.load,
                side: r.side,
                delay: r.delay,
                liberty_delay: r.liberty_delay,
                transition: r.transition,
                liberty_transition: r.liberty_transition,
            }),
            Err(e) => errors.push(format!("{} {}->{} {}: {e}", jobs[k].cell, jobs[k].input, jobs[k].output, if jobs[k].output_rise { "rise" } else { "fall" })),
        }
    }
    // Per-cell summary of the relative differences.
    #[derive(Serialize)]
    struct CellSummary {
        cell: String,
        samples: usize,
        delay_mean_abs_pct: f64,
        delay_max_abs_pct: f64,
        delay_mean_pct: f64,
        transition_mean_abs_pct: f64,
        transition_max_abs_pct: f64,
    }
    let mut summary = Vec::new();
    println!("{:<30} {:>7} {:>22} {:>22}", "cell", "samples", "delay vs Liberty", "transition vs Liberty");
    println!("{:<30} {:>7} {:>22} {:>22}", "", "", "mean |d|  max |d|  bias", "mean |d|  max |d|");
    for c in &cells {
        let s: Vec<&Sample> = samples.iter().filter(|x| &x.cell == c).collect();
        let d: Vec<f64> = s.iter().filter_map(|x| pct(x.delay, x.liberty_delay)).collect();
        let t: Vec<f64> = s.iter().filter_map(|x| pct(x.transition, x.liberty_transition)).collect();
        let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
        let mabs = |v: &[f64]| mean(&v.iter().map(|x| x.abs()).collect::<Vec<_>>());
        let max = |v: &[f64]| v.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        let cs = CellSummary {
            cell: c.clone(),
            samples: s.len(),
            delay_mean_abs_pct: mabs(&d),
            delay_max_abs_pct: max(&d),
            delay_mean_pct: mean(&d),
            transition_mean_abs_pct: mabs(&t),
            transition_max_abs_pct: max(&t),
        };
        println!(
            "{:<30} {:>7} {:>7.1}% {:>7.1}% {:>+5.1}% {:>9.1}% {:>7.1}%",
            c.trim_start_matches(PREFIX),
            cs.samples,
            cs.delay_mean_abs_pct,
            cs.delay_max_abs_pct,
            cs.delay_mean_pct,
            cs.transition_mean_abs_pct,
            cs.transition_max_abs_pct
        );
        summary.push(cs);
    }
    for e in &errors {
        println!("  error: {e}");
    }
    println!("{} samples in {:.1} s", samples.len(), t0.elapsed().as_secs_f64());
    if let Some(p) = o.flags.get("json") {
        #[derive(Serialize)]
        struct Out<'a> {
            category: &'static str,
            note: &'static str,
            slews_ns: &'a [f64],
            loads_pf: &'a [f64],
            cells: &'a [CellSummary],
            errors: &'a [String],
            samples: &'a [Sample],
        }
        write_json(
            p,
            &Out {
                category: "simulated",
                note: "ngspice on the PDK schematic netlists (no layout parasitics), typical corner, lumped load; \
                       Liberty values interpolated from sky130_fd_sc_hd__tt_025C_1v80",
                slews_ns: &slews,
                loads_pf: &loads,
                cells: &summary,
                errors: &errors,
                samples: &samples,
            },
        )?;
    }
    Ok(errors.is_empty())
}

fn one_arc(a: &Analyzer, o: &Opts) -> Result<bool, String> {
    let [cell, input, output, edge] = o.positional.as_slice() else { return Err(USAGE.into()) };
    let f = |k: &str, d: f64| o.flags.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
    let side = o
        .flags
        .get("side")
        .map(|s| {
            s.split(',')
                .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v == "1")))
                .collect()
        })
        .unwrap_or_default();
    let spec = ArcSpec {
        cell: full_name(cell),
        input: input.clone(),
        output: output.clone(),
        output_rise: edge == "rise",
        slew: f("slew", 0.05),
        load: f("load", 0.005),
        side,
    };
    let r = a.arc(&spec, &work().join("arc"))?;
    println!(
        "{} {} {} -> {} {} (slew {} ns, load {} pF, side {:?})",
        spec.cell,
        spec.input,
        if r.input_rise { "rise" } else { "fall" },
        spec.output,
        edge,
        spec.slew,
        spec.load,
        r.side
    );
    let show = |m: f64, l: Option<f64>| match l {
        Some(l) => format!("{m:.4} ns (Liberty {l:.4} ns, {:+.1}%)", 100.0 * (m - l) / l),
        None => format!("{m:.4} ns (no Liberty table)"),
    };
    println!("  delay      {}", show(r.delay, r.liberty_delay));
    println!("  transition {}", show(r.transition, r.liberty_transition));
    if let Some(p) = o.flags.get("csv") {
        let mut s = String::from("time_ns");
        for (n, _) in &r.waves.signals {
            s += &format!(",{n}");
        }
        s.push('\n');
        for (k, t) in r.waves.time.iter().enumerate() {
            s += &format!("{:.5}", t * 1e9);
            for (_, v) in &r.waves.signals {
                s += &format!(",{:.5}", v[k]);
            }
            s.push('\n');
        }
        std::fs::write(p, s).map_err(|e| e.to_string())?;
    }
    Ok(true)
}

fn models(a: &Analyzer, o: &Opts) -> Result<bool, String> {
    let cells: Vec<String> = o
        .flags
        .get("cells")
        .map(|c| c.split(',').map(full_name).collect())
        .unwrap_or_else(|| ["inv_1", "nand2_1", "mux4_2", "dfxtp_1", "edfxtp_1", "conb_1"].iter().map(|c| full_name(c)).collect());
    let reduced = a.verify(&cells, &work().join("models_cells"), Models::Cells)?;
    let full = a.verify(&cells, &work().join("models_full"), Models::Full)?;
    let mut ok = true;
    for (r, f) in reduced.iter().zip(&full) {
        let same = r.mismatches.len() == f.mismatches.len() && (r.min_high - f.min_high).abs() < 1e-6 && (r.max_low - f.max_low).abs() < 1e-6;
        println!("{:<34} reduced {:.6}/{:.6} V  full {:.6}/{:.6} V  {}", r.cell, r.min_high, r.max_low, f.min_high, f.max_low, if same { "same" } else { "DIFFERENT" });
        ok &= same;
    }
    for c in ["nand2_1", "dfxtp_1"] {
        let cell = full_name(c);
        let (input, output) = if c.starts_with("df") { ("CLK", "Q") } else { ("A", "Y") };
        let spec = ArcSpec { cell: cell.clone(), input: input.into(), output: output.into(), output_rise: true, slew: 0.1, load: 0.01, side: vec![] };
        let r = strata_spice::arc::simulate(&a.pdk, &a.lib, &a.cells, &spec, &work().join("models_arc_cells"), Models::Cells)?;
        let f = strata_spice::arc::simulate(&a.pdk, &a.lib, &a.cells, &spec, &work().join("models_arc_full"), Models::Full)?;
        let same = (r.delay - f.delay).abs() < 1e-9 && (r.transition - f.transition).abs() < 1e-9;
        println!("{cell:<34} delay {:.6}/{:.6} ns  transition {:.6}/{:.6} ns  {}", r.delay, f.delay, r.transition, f.transition, if same { "same" } else { "DIFFERENT" });
        ok &= same;
    }
    Ok(ok)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let run = || -> Result<bool, String> {
        let o = parse(&args[1..])?;
        let a = Analyzer::new()?;
        match cmd.as_str() {
            "verify" => verify(&a, &o),
            "characterize" => characterize(&a, &o),
            "arc" => one_arc(&a, &o),
            "models" => models(&a, &o),
            _ => Err(USAGE.into()),
        }
    };
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("strata-spice: {e}");
            ExitCode::from(2)
        }
    }
}
