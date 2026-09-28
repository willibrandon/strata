//! Total capacitance of each net from a SPEF file (extracted parasitics).

use std::collections::HashMap;

/// Net name -> total capacitance (pF).
pub fn read(text: &str) -> HashMap<String, f64> {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut scale = 1.0; // to pF
    let mut out = HashMap::new();
    let mut in_map = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("*C_UNIT") {
            let mut t = rest.split_whitespace();
            let v: f64 = t.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            scale = v * match t.next().unwrap_or("PF") {
                "FF" => 1e-3,
                "NF" => 1e3,
                _ => 1.0,
            };
        } else if line.starts_with("*NAME_MAP") {
            in_map = true;
        } else if in_map && line.starts_with('*') && !line.starts_with("*D_NET") && !line.starts_with("*PORTS") {
            let mut t = line.splitn(2, ' ');
            if let (Some(k), Some(v)) = (t.next(), t.next()) {
                names.insert(k.to_string(), v.trim().replace('\\', ""));
            }
        } else if let Some(rest) = line.strip_prefix("*D_NET") {
            in_map = false;
            let mut t = rest.split_whitespace();
            if let (Some(n), Some(c)) = (t.next(), t.next().and_then(|c| c.parse::<f64>().ok())) {
                let name = names.get(n).cloned().unwrap_or_else(|| n.replace('\\', ""));
                out.insert(name, c * scale);
            }
        } else if line.starts_with("*PORTS") || line.starts_with("*D_NET") {
            in_map = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn totals() {
        let m = super::read("*C_UNIT 1 PF\n*NAME_MAP\n*1 u_core/_n69_\n*2 clk\n\n*D_NET *1 0.0481056\n*CONN\n*END\n*D_NET *2 1.5\n");
        assert_eq!(m["u_core/_n69_"], 0.0481056);
        assert_eq!(m["clk"], 1.5);
    }
}
