//! The selected cell at transistor level: its netlist from the PDK drawn as
//! pull-up and pull-down networks per stage, which transistors conduct in the
//! current cycle (switch-level evaluation from the cell's pin values), and an
//! ngspice simulation of the transition the cell makes, with the load and
//! input transition taken from the layout, next to the Liberty prediction.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Pos2, RichText, Stroke};
use strata_netlist::PortDir;
use strata_spice::arc::{ArcResult, ArcSpec};
use strata_spice::{Analyzer, Subckt};

use crate::app::Statics;
use crate::view::View;
use crate::Selection;

/// Loads the analyzer (Liberty library and transistor netlists) in the
/// background, once.
pub type AnalyzerSlot = Arc<Mutex<Option<Result<Arc<Analyzer>, String>>>>;

pub fn start_loading() -> AnalyzerSlot {
    let slot: AnalyzerSlot = Arc::new(Mutex::new(None));
    let s = Arc::clone(&slot);
    std::thread::spawn(move || {
        let r = Analyzer::new().map(Arc::new);
        *s.lock().unwrap() = Some(r);
    });
    slot
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum V {
    L,
    H,
    X,
}

fn is_rail(n: &str) -> bool {
    matches!(n, "VPWR" | "VPB" | "VGND" | "VNB")
}

/// Conducting elements of a cell: (a, b, gate, pmos) for transistors; the
/// tie cell's resistors always conduct (gate None).
fn channels(sub: &Subckt) -> Vec<(String, String, Option<String>, bool)> {
    let mut v: Vec<_> = sub.mosfets.iter().map(|m| (m.drain.clone(), m.source.clone(), Some(m.gate.clone()), m.is_pmos())).collect();
    for o in &sub.other {
        let t: Vec<&str> = o.split_whitespace().collect();
        if t.len() >= 3 {
            v.push((t[1].to_string(), t[2].to_string(), None, false));
        }
    }
    v
}

/// Switch-level evaluation: node values from the input pin values, and
/// whether each channel conducts (None: unknown).
fn evaluate(sub: &Subckt, inputs: &HashMap<String, bool>) -> (HashMap<String, V>, Vec<Option<bool>>) {
    let ch = channels(sub);
    let mut val: HashMap<String, V> = HashMap::new();
    for (a, b, g, _) in &ch {
        for n in [a, b].into_iter().chain(g.as_ref()) {
            val.entry(n.clone()).or_insert(V::X);
        }
    }
    for r in ["VPWR", "VPB"] {
        val.insert(r.into(), V::H);
    }
    for r in ["VGND", "VNB"] {
        val.insert(r.into(), V::L);
    }
    for (p, &b) in inputs {
        val.insert(p.clone(), if b { V::H } else { V::L });
    }
    let fixed: HashSet<String> = val.keys().filter(|k| is_rail(k) || inputs.contains_key(*k)).cloned().collect();
    let mut on = vec![None; ch.len()];
    for _ in 0..24 {
        for (k, (_, _, g, pmos)) in ch.iter().enumerate() {
            on[k] = match g {
                None => Some(true),
                Some(g) => match val[g] {
                    V::H => Some(!pmos),
                    V::L => Some(*pmos),
                    V::X => None,
                },
            };
        }
        // Nets reachable from each rail through definitely (and possibly)
        // conducting channels.
        let reach = |from: &str, possible: bool| -> HashSet<String> {
            let mut seen: HashSet<String> = HashSet::new();
            let mut q = VecDeque::from([from.to_string()]);
            while let Some(n) = q.pop_front() {
                if !seen.insert(n.clone()) {
                    continue;
                }
                if n != from && fixed.contains(&n) {
                    continue; // inputs and the other rail do not pass values through
                }
                for (k, (a, b, _, _)) in ch.iter().enumerate() {
                    let conducts = on[k] == Some(true) || (possible && on[k].is_none());
                    if conducts {
                        if a == &n {
                            q.push_back(b.clone());
                        } else if b == &n {
                            q.push_back(a.clone());
                        }
                    }
                }
            }
            seen
        };
        let (h, hp, l, lp) = (reach("VPWR", false), reach("VPWR", true), reach("VGND", false), reach("VGND", true));
        let mut changed = false;
        for (n, v) in val.iter_mut() {
            if fixed.contains(n) {
                continue;
            }
            let new = if h.contains(n) && !lp.contains(n) {
                V::H
            } else if l.contains(n) && !hp.contains(n) {
                V::L
            } else {
                V::X
            };
            if new != *v {
                *v = new;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (val, on)
}

/// Channel-connected components (logic stages), in signal order.
fn stages(sub: &Subckt) -> Vec<Vec<usize>> {
    let ch = channels(sub);
    let mut parent: Vec<usize> = (0..ch.len()).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut x = x;
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let mut by_net: HashMap<&str, usize> = HashMap::new();
    for (k, (a, b, _, _)) in ch.iter().enumerate() {
        for n in [a, b] {
            if is_rail(n) {
                continue;
            }
            match by_net.get(n.as_str()) {
                Some(&j) => {
                    let (x, y) = (find(&mut parent, j), find(&mut parent, k));
                    parent[x] = y;
                }
                None => {
                    by_net.insert(n, k);
                }
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut root_group: HashMap<usize, usize> = HashMap::new();
    for k in 0..ch.len() {
        let r = find(&mut parent, k);
        let g = *root_group.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(k);
    }
    // Order: a stage driven only by cell inputs first, then the stages its
    // nets drive (feedback loops keep their original order).
    let nets_of = |g: &Vec<usize>| -> HashSet<String> { g.iter().flat_map(|&k| [ch[k].0.clone(), ch[k].1.clone()]).collect() };
    let gates_of = |g: &Vec<usize>| -> HashSet<String> { g.iter().filter_map(|&k| ch[k].2.clone()).collect() };
    let mut order: Vec<usize> = Vec::new();
    let mut placed = vec![false; groups.len()];
    while order.len() < groups.len() {
        let pick = (0..groups.len())
            .filter(|&i| !placed[i])
            .find(|&i| {
                let gi = gates_of(&groups[i]);
                (0..groups.len()).all(|j| j == i || placed[j] || nets_of(&groups[j]).is_disjoint(&gi))
            })
            .unwrap_or_else(|| (0..groups.len()).find(|&i| !placed[i]).unwrap());
        placed[pick] = true;
        order.push(pick);
    }
    order.into_iter().map(|i| groups[i].clone()).collect()
}

pub struct TransistorView {
    slot: AnalyzerSlot,
    job: Option<(u32, Receiver<Result<ArcResult, String>>)>,
    result: Option<(u32, Result<ArcResult, String>)>,
    input: String,
    output_rise: bool,
    slew: f64,
    load: f64,
    auto_side: bool,
    for_cell: Option<u32>,
    /// Simulate as soon as a cell and the analyzer are ready (screenshots).
    pub auto_run: bool,
}

impl TransistorView {
    pub fn new(slot: AnalyzerSlot) -> TransistorView {
        TransistorView {
            slot,
            job: None,
            result: None,
            input: String::new(),
            output_rise: true,
            slew: 0.1,
            load: 0.005,
            auto_side: false,
            for_cell: None,
            auto_run: false,
        }
    }

    /// Loading the analyzer, or a simulation in progress.
    pub fn busy(&self) -> bool {
        self.slot.lock().unwrap().is_none() || self.job.is_some() || (self.auto_run && self.result.is_none())
    }

    /// Picks the arc the cell exercised in the current cycle, with its load
    /// and input transition from the layout.
    fn defaults(&mut self, st: &Statics, view: &View, a: &Analyzer, cell: u32) {
        let c = &st.compiled;
        let cl = &c.netlist.cells[cell as usize];
        let def = &c.defs[c.cell_def[cell as usize] as usize];
        let changed: HashSet<u32> = view.changed.iter().copied().collect();
        let value = |net: u32| view.nets.get(net as usize).copied().unwrap_or(0) != 0;
        let out = cl.pins.iter().find(|p| p.dir == PortDir::Output);
        self.input = match &def.seq {
            Some(seq) => seq.clock_pin.clone(),
            None => cl
                .pins
                .iter()
                .find(|p| p.dir == PortDir::Input && changed.contains(&p.net))
                .or_else(|| cl.pins.iter().find(|p| p.dir == PortDir::Input))
                .map(|p| p.name.clone())
                .unwrap_or_default(),
        };
        self.output_rise = out.is_none_or(|p| value(p.net));
        // With the current side inputs the input may not control the output;
        // then let the simulator choose a sensitizing condition.
        self.auto_side = def.seq.is_some() || out.is_none_or(|p| !changed.contains(&p.net));
        self.load = 0.005;
        self.slew = 0.1;
        if let Some(ph) = st.physical.as_deref() {
            if let Some(o) = out.and_then(|p| ph.net_of(cell, &p.name)) {
                self.load = ph.load_of(&a.lib, o);
            }
            let in_rise = cl.pins.iter().find(|p| p.name == self.input).is_some_and(|p| value(p.net));
            if let Some(i) = ph.net_of(cell, &self.input) {
                if let Some(s) = ph.slew_of(&a.lib, i, in_rise) {
                    self.slew = s;
                }
            }
        } else if let Some(o) = out {
            // No layout: the input capacitance of the reading pins.
            self.load = strata_debug::inspect::reader_cells(c, o.net)
                .iter()
                .flat_map(|&r| c.netlist.cells[r as usize].pins.iter().filter(|p| p.net == o.net && p.dir == PortDir::Input).map(move |p| (r, p)))
                .filter_map(|(r, p)| strata_spice::timing::pin_cap(&a.lib, &c.netlist.cells[r as usize].ty, &p.name))
                .sum::<f64>()
                .max(0.0005);
        }
        self.for_cell = Some(cell);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, st: &Statics, view: &View, sel: &Selection) {
        let Some(cell) = sel.cell else {
            ui.label("Select a cell (3D view, layout, or inspector) to see it at transistor level.");
            return;
        };
        let loaded = self.slot.lock().unwrap().clone();
        let a = match loaded {
            None => {
                ui.label("Loading the Liberty library and the PDK transistor netlists...");
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                return;
            }
            Some(Err(e)) => {
                ui.label(format!("Transistor-level analysis unavailable: {e}"));
                return;
            }
            Some(Ok(a)) => a,
        };
        let c = &st.compiled;
        let cl = &c.netlist.cells[cell as usize];
        let Some(sub) = a.cells.get(&cl.ty) else {
            ui.label(format!("{}: no transistor netlist in the PDK", cl.ty));
            return;
        };
        if self.for_cell != Some(cell) {
            self.defaults(st, view, &a, cell);
            self.result = None;
        }
        let value = |net: u32| view.nets.get(net as usize).copied().unwrap_or(0) != 0;
        let inputs: HashMap<String, bool> = cl.pins.iter().filter(|p| p.dir == PortDir::Input).map(|p| (p.name.clone(), value(p.net))).collect();
        let (vals, on) = evaluate(sub, &inputs);

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.label(RichText::new(format!("{}  {}", cl.name, cl.ty)).monospace().strong());
            let np = sub.mosfets.iter().filter(|m| m.is_pmos()).count();
            ui.label(format!(
                "{} transistors ({} pfet_01v8_hvt, {} nfet), from the PDK's {}",
                sub.mosfets.len(),
                np,
                sub.mosfets.len() - np,
                a.pdk.cell_spice().file_name().unwrap_or_default().to_string_lossy()
            ));
            // Switch-level result against the gate model.
            let mut agree = true;
            for p in cl.pins.iter().filter(|p| p.dir == PortDir::Output) {
                let sw = vals.get(&p.name).copied().unwrap_or(V::X);
                let gate = value(p.net);
                let txt = match sw {
                    V::X => format!("{} = X at switch level (stored state), gate model {}", p.name, gate as u8),
                    v => {
                        let ok = (v == V::H) == gate;
                        agree &= ok;
                        format!("{} = {} at switch level, {} in the gate model{}", p.name, (v == V::H) as u8, gate as u8, if ok { "" } else { "  (DIFFERENT)" })
                    }
                };
                ui.label(RichText::new(txt).monospace().color(if agree { Color32::LIGHT_GREEN } else { Color32::LIGHT_RED }));
            }
            self.schematic(ui, sub, &vals, &on);
            ui.collapsing("devices", |ui| {
                egui::Grid::new("fets").striped(true).show(ui, |ui| {
                    for h in ["name", "type", "drain", "gate", "source", "W µm", "L µm", "conducts"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for (k, m) in sub.mosfets.iter().enumerate() {
                        ui.label(RichText::new(&m.name).monospace());
                        ui.label(if m.is_pmos() { "pmos" } else { "nmos" });
                        ui.label(RichText::new(&m.drain).monospace());
                        ui.label(RichText::new(&m.gate).monospace());
                        ui.label(RichText::new(&m.source).monospace());
                        ui.label(format!("{:.2}", m.w));
                        ui.label(format!("{:.2}", m.l));
                        ui.label(match on[k] {
                            Some(true) => "on",
                            Some(false) => "off",
                            None => "?",
                        });
                        ui.end_row();
                    }
                });
            });
            ui.separator();
            self.simulation(ui, st, view, &a, cell);
        });
    }

    fn schematic(&self, ui: &mut egui::Ui, sub: &Subckt, vals: &HashMap<String, V>, on: &[Option<bool>]) {
        let ch = channels(sub);
        let groups = stages(sub);
        let slot = 34.0;
        let widths: Vec<f32> = groups
            .iter()
            .map(|g| {
                let p = g.iter().filter(|&&k| ch[k].3).count();
                let n = g.len() - p;
                (p.max(n).max(1) as f32) * slot + 36.0
            })
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + 20.0;
        let height = 240.0;
        egui::ScrollArea::horizontal().id_salt("schematic").show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(total.max(ui.available_width()), height), egui::Sense::hover());
            let p = ui.painter_at(rect);
            let top = rect.top() + 22.0;
            let bot = rect.bottom() - 22.0;
            let mid = (top + bot) / 2.0;
            let col = |v: V| match v {
                V::H => Color32::from_rgb(250, 210, 80),
                V::L => Color32::from_rgb(90, 120, 200),
                V::X => Color32::from_gray(130),
            };
            p.line_segment([Pos2::new(rect.left() + 4.0, top), Pos2::new(rect.left() + total, top)], Stroke::new(2.0, col(V::H)));
            p.line_segment([Pos2::new(rect.left() + 4.0, bot), Pos2::new(rect.left() + total, bot)], Stroke::new(2.0, col(V::L)));
            p.text(Pos2::new(rect.left() + 6.0, top - 10.0), egui::Align2::LEFT_CENTER, "VPWR", egui::FontId::monospace(10.0), Color32::GRAY);
            p.text(Pos2::new(rect.left() + 6.0, bot + 10.0), egui::Align2::LEFT_CENTER, "VGND", egui::FontId::monospace(10.0), Color32::GRAY);
            let mut x0 = rect.left() + 20.0;
            for (g, w) in groups.iter().zip(&widths) {
                // Net heights: distance from the rail through this stage's
                // pull-up (pmos) or pull-down (nmos) network.
                let dist = |from: &str, pmos: bool| -> HashMap<String, usize> {
                    let mut d = HashMap::from([(from.to_string(), 0usize)]);
                    let mut q = VecDeque::from([from.to_string()]);
                    while let Some(n) = q.pop_front() {
                        for &k in g {
                            if ch[k].3 != pmos {
                                continue;
                            }
                            let other = if ch[k].0 == n { &ch[k].1 } else if ch[k].1 == n { &ch[k].0 } else { continue };
                            if !d.contains_key(other) && !is_rail(other) {
                                d.insert(other.clone(), d[&n] + 1);
                                q.push_back(other.clone());
                            }
                        }
                    }
                    d
                };
                let (dp, dn) = (dist("VPWR", true), dist("VGND", false));
                let touch = |n: &str, pmos: bool| g.iter().any(|&k| ch[k].3 == pmos && (ch[k].0 == n || ch[k].1 == n));
                let maxp = dp.values().copied().max().unwrap_or(1).max(1) as f32;
                let maxn = dn.values().copied().max().unwrap_or(1).max(1) as f32;
                let y_of = |n: &str| -> f32 {
                    match n {
                        "VPWR" | "VPB" => top,
                        "VGND" | "VNB" => bot,
                        _ if touch(n, true) && touch(n, false) => mid,
                        _ if dp.contains_key(n) => top + (mid - top) * dp[n] as f32 / (maxp + 1.0),
                        _ if dn.contains_key(n) => bot - (bot - mid) * dn[n] as f32 / (maxn + 1.0),
                        _ => mid,
                    }
                };
                let mut span: HashMap<String, (f32, f32)> = HashMap::new();
                let (mut ip, mut inn) = (0, 0);
                for &k in g {
                    let (a, b, gate, pmos) = &ch[k];
                    let slot_i = if *pmos {
                        ip += 1;
                        ip
                    } else {
                        inn += 1;
                        inn
                    };
                    let x = x0 + 18.0 + (slot_i - 1) as f32 * slot + 10.0;
                    let (ya, yb) = (y_of(a), y_of(b));
                    let state = on.get(k).copied().flatten();
                    let c = match state {
                        Some(true) => Color32::from_rgb(120, 230, 120),
                        Some(false) => Color32::from_gray(70),
                        None => Color32::from_rgb(120, 140, 170),
                    };
                    p.line_segment([Pos2::new(x, ya), Pos2::new(x, yb)], Stroke::new(if state == Some(true) { 3.0 } else { 1.5 }, c));
                    let ym = (ya + yb) / 2.0;
                    if let Some(gn) = gate {
                        p.line_segment([Pos2::new(x - 7.0, ym - 6.0), Pos2::new(x - 7.0, ym + 6.0)], Stroke::new(1.5, col(vals.get(gn).copied().unwrap_or(V::X))));
                        if *pmos {
                            p.circle_stroke(Pos2::new(x - 10.0, ym), 2.5, Stroke::new(1.0, Color32::GRAY));
                        }
                        let short: String = gn.chars().take(6).collect();
                        p.text(Pos2::new(x - 13.0, ym), egui::Align2::RIGHT_CENTER, short, egui::FontId::monospace(9.0), col(vals.get(gn).copied().unwrap_or(V::X)));
                    } else {
                        p.text(Pos2::new(x + 3.0, ym), egui::Align2::LEFT_CENTER, "R", egui::FontId::monospace(9.0), Color32::GRAY);
                    }
                    for n in [a, b] {
                        if !is_rail(n) {
                            let e = span.entry(n.clone()).or_insert((x, x));
                            e.0 = e.0.min(x);
                            e.1 = e.1.max(x);
                        }
                    }
                }
                for (n, (xa, xb)) in &span {
                    let y = y_of(n);
                    let v = vals.get(n).copied().unwrap_or(V::X);
                    p.line_segment([Pos2::new(xa - 4.0, y), Pos2::new(xb + 4.0, y)], Stroke::new(2.0, col(v)));
                    let port = sub.ports.contains(n);
                    if port || span.len() <= 4 {
                        let label = if port { n.clone() } else { n.chars().take(8).collect() };
                        p.text(Pos2::new(xb + 6.0, y - 7.0), egui::Align2::LEFT_CENTER, label, egui::FontId::monospace(if port { 11.0 } else { 9.0 }), col(v));
                    }
                }
                x0 += w;
            }
            p.text(
                Pos2::new(rect.right() - 6.0, rect.bottom() - 6.0),
                egui::Align2::RIGHT_BOTTOM,
                "green: conducting this cycle   yellow/blue: node at 1/0   gray: unknown (stored state)",
                egui::FontId::proportional(10.0),
                Color32::GRAY,
            );
        });
    }

    fn simulation(&mut self, ui: &mut egui::Ui, st: &Statics, view: &View, a: &Arc<Analyzer>, cell: u32) {
        let c = &st.compiled;
        let cl = &c.netlist.cells[cell as usize];
        let def = &c.defs[c.cell_def[cell as usize] as usize];
        let Some(out) = cl.pins.iter().find(|p| p.dir == PortDir::Output) else { return };
        ui.label(RichText::new("Transistor-level simulation (ngspice, sky130 tt corner)").strong());
        ui.horizontal(|ui| {
            ui.label("input");
            egui::ComboBox::from_id_salt("arc_in").selected_text(&self.input).show_ui(ui, |ui| {
                let choices: Vec<String> = match &def.seq {
                    Some(s) => vec![s.clock_pin.clone()],
                    None => def.inputs.clone(),
                };
                for i in choices {
                    ui.selectable_value(&mut self.input, i.clone(), i);
                }
            });
            ui.label(format!("output {}", out.name));
            ui.selectable_value(&mut self.output_rise, true, "rises");
            ui.selectable_value(&mut self.output_rise, false, "falls");
        });
        ui.horizontal(|ui| {
            ui.label("input transition (ns)");
            ui.add(egui::DragValue::new(&mut self.slew).speed(0.005).range(0.005..=1.5).max_decimals(4));
            ui.label("load (pF)");
            ui.add(egui::DragValue::new(&mut self.load).speed(0.0005).range(0.0002..=0.2).max_decimals(5));
            ui.checkbox(&mut self.auto_side, "choose other inputs").on_hover_text(
                "off: the other inputs keep their values in this cycle; on: any values under which the input controls the output",
            );
        });
        ui.label(
            RichText::new(if st.physical.is_some() {
                "Defaults: the transition this cell made in this cycle; load = extracted wire capacitance (SPEF) + input pins on the routed net; input transition from the driver's Liberty table at its load."
            } else {
                "Defaults: the transition this cell made in this cycle; load = input pins of the readers (no layout loaded)."
            })
            .weak(),
        );
        let running = self.job.is_some();
        let auto = self.auto_run && self.result.is_none() && !running;
        if ui.add_enabled(!running, egui::Button::new(if running { "simulating..." } else { "simulate" })).clicked() || auto {
            let side = if self.auto_side || def.seq.is_some() {
                Vec::new()
            } else {
                cl.pins
                    .iter()
                    .filter(|p| p.dir == PortDir::Input && p.name != self.input)
                    .map(|p| (p.name.clone(), view.nets.get(p.net as usize).copied().unwrap_or(0) != 0))
                    .collect()
            };
            let spec = ArcSpec {
                cell: cl.ty.clone(),
                input: self.input.clone(),
                output: out.name.clone(),
                output_rise: self.output_rise,
                slew: self.slew,
                load: self.load,
                side,
            };
            let (tx, rx) = channel();
            let a = Arc::clone(a);
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let r = a.arc(&spec, std::path::Path::new("build/spice/dbg"));
                let _ = tx.send(r);
                ctx.request_repaint();
            });
            self.job = Some((cell, rx));
        }
        if let Some((c0, rx)) = &self.job {
            if let Ok(r) = rx.try_recv() {
                self.result = Some((*c0, r));
                self.job = None;
            }
        }
        match &self.result {
            Some((c0, Ok(r))) if *c0 == cell => plot(ui, r),
            Some((c0, Err(e))) if *c0 == cell => {
                ui.label(RichText::new(e).color(Color32::LIGHT_RED));
            }
            _ => {}
        }
    }
}

fn plot(ui: &mut egui::Ui, r: &ArcResult) {
    let show = |m: f64, l: Option<f64>| match l {
        Some(l) => format!("{:.1} ps (Liberty {:.1} ps, {:+.0}%)", m * 1000.0, l * 1000.0, 100.0 * (m - l) / l),
        None => format!("{:.1} ps", m * 1000.0),
    };
    ui.label(RichText::new(format!("delay       {}", show(r.delay, r.liberty_delay))).monospace());
    ui.label(RichText::new(format!("transition  {}", show(r.transition, r.liberty_transition))).monospace());
    ui.label(
        RichText::new(format!(
            "{} {} {} -> {} {}, other inputs {:?}",
            r.spec.cell.trim_start_matches("sky130_fd_sc_hd__"),
            r.spec.input,
            if r.input_rise { "rises" } else { "falls" },
            r.spec.output,
            if r.spec.output_rise { "rises" } else { "falls" },
            r.side
        ))
        .weak(),
    );
    let w = &r.waves;
    if w.time.is_empty() || w.signals.len() < 2 {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 190.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, Color32::from_rgb(16, 18, 24));
    // Window: from shortly before the input edge to when the output settles.
    let vin = &w.signals[0].1;
    let vout = &w.signals[1].1;
    let t_in = strata_spice::arc::crossing(&w.time, vin, 0.9, r.input_rise, 0.0).unwrap_or(w.time[0]);
    let t0 = (t_in - 0.3e-9).max(w.time[0]);
    let t1 = (t_in + (r.delay + 2.5 * r.transition).max(0.3) * 1e-9 + 0.2e-9).min(*w.time.last().unwrap());
    let x = |t: f64| rect.left() + 40.0 + (rect.width() - 50.0) * ((t - t0) / (t1 - t0)) as f32;
    let y = |v: f64| rect.bottom() - 18.0 - (rect.height() - 30.0) * (v / 1.8) as f32;
    for v in [0.0, 0.9, 1.8] {
        p.line_segment([Pos2::new(x(t0), y(v)), Pos2::new(x(t1), y(v))], Stroke::new(0.5, Color32::from_gray(60)));
        p.text(Pos2::new(rect.left() + 4.0, y(v)), egui::Align2::LEFT_CENTER, format!("{v:.1} V"), egui::FontId::monospace(9.0), Color32::GRAY);
    }
    for (vals, col, name) in [(vin, Color32::from_rgb(120, 180, 255), &r.spec.input), (vout, Color32::from_rgb(255, 170, 60), &r.spec.output)] {
        let pts: Vec<Pos2> = w.time.iter().zip(vals).filter(|(t, _)| **t >= t0 && **t <= t1).map(|(t, v)| Pos2::new(x(*t), y(*v))).collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.8, col)));
        p.text(Pos2::new(x(t1) - 4.0, y(if vals.last().copied().unwrap_or(0.0) > 0.9 { 1.7 } else { 0.1 })), egui::Align2::RIGHT_CENTER, name, egui::FontId::monospace(10.0), col);
    }
    // Measured and Liberty output crossing times.
    let tm = t_in + r.delay * 1e-9;
    p.line_segment([Pos2::new(x(tm), rect.top() + 6.0), Pos2::new(x(tm), rect.bottom() - 18.0)], Stroke::new(1.0, Color32::from_rgb(255, 170, 60)));
    if let Some(ld) = r.liberty_delay {
        let tl = t_in + ld * 1e-9;
        let mut yy = rect.top() + 6.0;
        while yy < rect.bottom() - 18.0 {
            p.line_segment([Pos2::new(x(tl), yy), Pos2::new(x(tl), yy + 4.0)], Stroke::new(1.0, Color32::WHITE));
            yy += 8.0;
        }
        p.text(Pos2::new(x(tl) + 3.0, rect.top() + 10.0), egui::Align2::LEFT_CENTER, "Liberty", egui::FontId::proportional(10.0), Color32::WHITE);
    }
    p.text(
        Pos2::new(x(t0), rect.bottom() - 6.0),
        egui::Align2::LEFT_CENTER,
        format!("{:.0} ps", (t0 - t_in) * 1e12),
        egui::FontId::monospace(9.0),
        Color32::GRAY,
    );
    p.text(
        Pos2::new(x(t1), rect.bottom() - 6.0),
        egui::Align2::RIGHT_CENTER,
        format!("+{:.0} ps from the input's 50% point", (t1 - t_in) * 1e12),
        egui::FontId::monospace(9.0),
        Color32::GRAY,
    );
}
