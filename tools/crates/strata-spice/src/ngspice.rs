//! Running ngspice in batch mode and reading its results.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use serde::Serialize;

/// The ngspice executable (`NGSPICE` overrides).
pub fn executable() -> String {
    std::env::var("NGSPICE").unwrap_or_else(|_| "ngspice".into())
}

/// Sampled node voltages of a transient analysis.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Waves {
    /// Time points (s).
    pub time: Vec<f64>,
    /// (vector name, values) for each saved vector.
    pub signals: Vec<(String, Vec<f64>)>,
}

impl Waves {
    pub fn get(&self, name: &str) -> Option<&[f64]> {
        self.signals.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_slice())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Output {
    pub log: String,
    /// `.meas` results by (lower-case) name.
    pub measures: HashMap<String, f64>,
    /// Measurements that ngspice could not make.
    pub failed: Vec<String>,
    pub waves: Option<Waves>,
}

/// Runs `deck` in `dir`. With `raw`, the vectors the deck saves are read
/// back from an ASCII raw file.
pub fn run(deck: &str, dir: &Path, raw: bool) -> Result<Output, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(dir.join(".spiceinit"), "set ngbehavior=hsa\nset ng_nomodcheck\nset filetype=ascii\nset num_threads=1\n")
        .map_err(|e| e.to_string())?;
    std::fs::write(dir.join("deck.sp"), deck).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(dir.join("out.raw"));
    let mut cmd = Command::new(executable());
    cmd.current_dir(dir).arg("-b");
    if raw {
        cmd.args(["-r", "out.raw"]);
    }
    cmd.arg("deck.sp");
    let out = cmd.output().map_err(|e| format!("running {}: {e}", executable()))?;
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    std::fs::write(dir.join("ngspice.log"), &log).map_err(|e| e.to_string())?;
    if !out.status.success() || log.contains("Error") {
        let first = log.lines().filter(|l| l.contains("rror") || l.contains("Mismatch")).take(4).collect::<Vec<_>>().join("\n");
        return Err(format!("ngspice failed ({}):\n{first}", dir.join("ngspice.log").display()));
    }
    let (measures, failed) = parse_measures(&log);
    let waves = if raw {
        let text = std::fs::read_to_string(dir.join("out.raw")).map_err(|e| format!("reading raw file: {e}"))?;
        Some(parse_raw(&text)?)
    } else {
        None
    };
    Ok(Output { log, measures, failed, waves })
}

/// `.meas` result lines: `name = value ...` or `name = failed`.
pub fn parse_measures(log: &str) -> (HashMap<String, f64>, Vec<String>) {
    let mut values = HashMap::new();
    let mut failed = Vec::new();
    for line in log.lines() {
        let Some((name, rest)) = line.split_once('=') else { continue };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let rest = rest.trim();
        if rest.starts_with("failed") {
            failed.push(name.to_ascii_lowercase());
        } else if let Some(v) = rest.split_whitespace().next().and_then(|t| t.parse::<f64>().ok()) {
            values.entry(name.to_ascii_lowercase()).or_insert(v);
        }
    }
    (values, failed)
}

/// Reads an ASCII raw file with one real-valued plot.
pub fn parse_raw(text: &str) -> Result<Waves, String> {
    let mut names = Vec::new();
    let mut lines = text.lines();
    let mut nvars = 0usize;
    while let Some(line) = lines.next() {
        if let Some(n) = line.strip_prefix("No. Variables:") {
            nvars = n.trim().parse().map_err(|_| "bad variable count")?;
        } else if line.starts_with("Variables:") {
            for _ in 0..nvars {
                let l = lines.next().ok_or("truncated variable list")?;
                let mut t = l.split_whitespace();
                t.next();
                names.push(t.next().ok_or("bad variable line")?.to_string());
            }
        } else if line.starts_with("Values:") {
            break;
        }
    }
    if names.is_empty() {
        return Err("raw file has no variables".into());
    }
    let nums: Vec<f64> = lines.flat_map(|l| l.split_whitespace()).filter_map(|t| t.parse().ok()).collect();
    let stride = nvars + 1; // point index, then one value per variable
    let mut w = Waves { time: Vec::new(), signals: names[1..].iter().map(|n| (n.clone(), Vec::new())).collect() };
    for chunk in nums.chunks_exact(stride) {
        w.time.push(chunk[1]);
        for (k, s) in w.signals.iter_mut().enumerate() {
            s.1.push(chunk[2 + k]);
        }
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures() {
        let (m, f) = parse_measures("tphl                =  4.17982e-11 targ=  1.06680e-09\nx = failed\nNote: a = b\n");
        assert_eq!(m["tphl"], 4.17982e-11);
        assert_eq!(f, ["x"]);
    }

    #[test]
    fn raw() {
        let w = parse_raw(
            "Title: t\nNo. Variables: 3\nNo. Points: 2\nVariables:\n\t0\ttime\ttime\n\t1\tv(a)\tvoltage\n\t2\tv(y)\tvoltage\nValues:\n 0\t0.0\n\t0.0\n\t1.8\n 1\t1e-12\n\t0.1\n\t1.7\n",
        )
        .unwrap();
        assert_eq!(w.time, [0.0, 1e-12]);
        assert_eq!(w.get("v(y)").unwrap(), [1.8, 1.7]);
    }
}
