//! Reading a placed and routed DEF: die, rows, components, I/O pins, and the
//! routed geometry of every signal net.

use std::collections::HashMap;

use crate::lef::Lef;
use crate::{Component, IoPin, Layout, Net, Via, Wire};

/// Removes DEF escapes (`\[` -> `[`).
fn unescape(s: &str) -> String {
    s.replace('\\', "")
}

struct Tokens<'a> {
    t: Vec<&'a str>,
    i: usize,
}

impl<'a> Tokens<'a> {
    fn next(&mut self) -> Option<&'a str> {
        let v = self.t.get(self.i).copied();
        self.i += 1;
        v
    }
    fn peek(&self) -> Option<&'a str> {
        self.t.get(self.i).copied()
    }
    /// Skips to just past the next `;`.
    fn skip_statement(&mut self) {
        while let Some(t) = self.next() {
            if t == ";" {
                break;
            }
        }
    }
    fn num(&mut self) -> f64 {
        self.next().and_then(|t| t.parse().ok()).unwrap_or(0.0)
    }
}

pub fn read(text: &str, lef: &Lef) -> Result<Layout, String> {
    let mut tk = Tokens { t: text.split_whitespace().collect(), i: 0 };
    let mut l = Layout { layers: lef.layers.clone(), ..Default::default() };
    let mut dbu = 1000.0;
    // Generated vias of the DEF (name -> layer pair).
    let mut vias: HashMap<String, (u8, u8)> = lef.vias.clone();
    while let Some(t) = tk.next() {
        match t {
            "DESIGN" if l.design.is_empty() => {
                l.design = tk.next().unwrap_or_default().to_string();
                tk.skip_statement();
            }
            "UNITS" => {
                tk.next();
                tk.next();
                dbu = tk.num();
                tk.skip_statement();
            }
            "DIEAREA" => {
                let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                while let Some(t) = tk.next() {
                    if t == ";" {
                        break;
                    }
                    if t == "(" {
                        let (x, y) = (tk.num() / dbu, tk.num() / dbu);
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                        tk.next();
                    }
                }
                l.die = [x0, y0, x1, y1];
            }
            "ROW" => {
                // ROW name site x y orient DO n BY 1 STEP sx sy ;
                let _name = tk.next();
                let site = tk.next().unwrap_or_default();
                let (x, y) = (tk.num() / dbu, tk.num() / dbu);
                let _orient = tk.next();
                tk.next();
                let n = tk.num();
                tk.next();
                tk.next();
                tk.next();
                let step = tk.num() / dbu;
                tk.skip_statement();
                let h = lef.macros.get(site).map_or(2.72, |m| m.1);
                l.rows.push([x, y, x + n * step, h]);
            }
            "VIAS" if tk.peek().is_some_and(|p| p.parse::<usize>().is_ok()) => {
                tk.skip_statement();
                // - name ... + LAYERS bot cut top ... ;   or rect-based vias
                loop {
                    match tk.next() {
                        Some("-") => {
                            let name = tk.next().unwrap_or_default().to_string();
                            let mut used = Vec::new();
                            while let Some(t) = tk.next() {
                                if t == ";" {
                                    break;
                                }
                                if let Some(i) = l.layers.iter().position(|x| x.name == t) {
                                    used.push(i as u8);
                                }
                            }
                            if let (Some(lo), Some(hi)) = (used.iter().min(), used.iter().max()) {
                                vias.insert(name, (*lo, *hi));
                            }
                        }
                        Some("END") => {
                            tk.next();
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
            "COMPONENTS" => {
                tk.skip_statement();
                loop {
                    match tk.next() {
                        Some("-") => {
                            let name = unescape(tk.next().unwrap_or_default());
                            let cell = tk.next().unwrap_or_default().to_string();
                            let mut c = Component { name, cell, ..Default::default() };
                            while let Some(t) = tk.next() {
                                match t {
                                    ";" => break,
                                    "DIST" => c.physical_only = true,
                                    "PLACED" | "FIXED" => {
                                        tk.next();
                                        c.x = tk.num() / dbu;
                                        c.y = tk.num() / dbu;
                                        tk.next();
                                        c.orient = tk.next().unwrap_or("N").to_string();
                                    }
                                    _ => {}
                                }
                            }
                            let (w, h) = lef.macros.get(&c.cell).copied().unwrap_or((0.46, 2.72));
                            (c.w, c.h) = if matches!(c.orient.as_str(), "E" | "W" | "FE" | "FW") { (h, w) } else { (w, h) };
                            l.components.push(c);
                        }
                        Some("END") => {
                            tk.next();
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
            "PINS" => {
                tk.skip_statement();
                loop {
                    match tk.next() {
                        Some("-") => {
                            let mut p = IoPin { name: unescape(tk.next().unwrap_or_default()), ..Default::default() };
                            while let Some(t) = tk.next() {
                                match t {
                                    ";" => break,
                                    "NET" => p.net = unescape(tk.next().unwrap_or_default()),
                                    "LAYER" => p.layer = l.layer_index(tk.next().unwrap_or_default()).unwrap_or(0),
                                    "PLACED" | "FIXED" => {
                                        tk.next();
                                        p.x = tk.num() / dbu;
                                        p.y = tk.num() / dbu;
                                    }
                                    _ => {}
                                }
                            }
                            l.pins.push(p);
                        }
                        Some("END") => {
                            tk.next();
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
            "NETS" => {
                tk.skip_statement();
                loop {
                    match tk.next() {
                        Some("-") => {
                            let net = read_net(&mut tk, &l, &vias, dbu);
                            l.nets.push(net);
                        }
                        Some("END") => {
                            tk.next();
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
            "SPECIALNETS" => {
                // Power and ground: counted, not kept.
                tk.skip_statement();
                loop {
                    match tk.next() {
                        Some("END") => {
                            tk.next();
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    l.dbu = dbu;
    l.index();
    Ok(l)
}

fn read_net(tk: &mut Tokens, l: &Layout, vias: &HashMap<String, (u8, u8)>, dbu: f64) -> Net {
    let mut net = Net { name: unescape(tk.next().unwrap_or_default()), ..Default::default() };
    // Connections: ( comp pin ) ...
    while tk.peek() == Some("(") {
        tk.next();
        let comp = unescape(tk.next().unwrap_or_default());
        let pin = unescape(tk.next().unwrap_or_default());
        tk.next(); // )
        net.pins.push((comp, pin));
    }
    let mut layer: u8 = 0;
    let mut last: Option<(f64, f64)> = None;
    let mut in_route = false;
    while let Some(t) = tk.next() {
        match t {
            ";" => break,
            "ROUTED" | "FIXED" | "COVER" | "NOSHIELD" | "NEW" => {
                if let Some(name) = tk.next() {
                    layer = l.layer_index(name).unwrap_or(layer);
                }
                last = None;
                in_route = true;
            }
            "+" => in_route = false,
            "(" if in_route => {
                let xs = tk.next().unwrap_or("*");
                let ys = tk.next().unwrap_or("*");
                // Optional extension value, then ")".
                let mut t = tk.next();
                if t != Some(")") {
                    t = tk.next();
                }
                let _ = t;
                let (px, py) = last.unwrap_or((0.0, 0.0));
                let x = if xs == "*" { px } else { xs.parse::<f64>().unwrap_or(0.0) / dbu };
                let y = if ys == "*" { py } else { ys.parse::<f64>().unwrap_or(0.0) / dbu };
                if let Some((px, py)) = last {
                    if (px, py) != (x, y) {
                        let w = l.layers.get(layer as usize).map_or(0.14, |x| x.width) as f32;
                        net.wires.push(Wire { layer, x0: px as f32, y0: py as f32, x1: x as f32, y1: y as f32, width: w });
                    }
                }
                last = Some((x, y));
            }
            "RECT" if in_route => {
                // A patch relative to the last point: ( dx0 dy0 dx1 dy1 ).
                tk.next();
                let v: Vec<f64> = (0..4).map(|_| tk.num() / dbu).collect();
                tk.next();
                if let Some((x, y)) = last {
                    let (cx, cy) = (x + (v[0] + v[2]) / 2.0, y + (v[1] + v[3]) / 2.0);
                    let (w, h) = (v[2] - v[0], v[3] - v[1]);
                    let (x0, y0, x1, y1, width) =
                        if w >= h { (cx - w / 2.0, cy, cx + w / 2.0, cy, h) } else { (cx, cy - h / 2.0, cx, cy + h / 2.0, w) };
                    net.wires.push(Wire { layer, x0: x0 as f32, y0: y0 as f32, x1: x1 as f32, y1: y1 as f32, width: width as f32 });
                }
            }
            "VIRTUAL" if in_route => {
                tk.next();
                let x = tk.num() / dbu;
                let y = tk.num() / dbu;
                tk.next();
                last = Some((x, y));
            }
            "MASK" | "STYLE" if in_route => {
                tk.next();
            }
            "TAPER" | "TAPERRULE" if in_route => {}
            other if in_route && last.is_some() => {
                // A via at the last point; the path continues on its other layer.
                if let Some(&(lo, hi)) = vias.get(other) {
                    let (x, y) = last.unwrap();
                    net.vias.push(Via { lower: lo, x: x as f32, y: y as f32 });
                    layer = if layer == lo { hi } else { lo };
                }
            }
            _ => {}
        }
    }
    net
}
