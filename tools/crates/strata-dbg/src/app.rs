//! The debugger application: layout, toolbar, and panels.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};
use eframe::egui_wgpu;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use strata_debug::inspect::{self, ModuleInfo, StageMap};
use strata_debug::{
    CmpOp, Event, FaultAction, InputEvent, Loc, MemId, Signal, TimelineEvent, TriggerKind,
};
use strata_gate::Compiled;

use crate::layout_view::LayoutView;
use crate::physical::Physical;
use crate::scene_view::{SceneView, STAGE_COLORS};
use crate::transistor_view::{self, TransistorView};
use crate::view::View;
use crate::worker::{Command, Worker};
use crate::Selection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Scene,
    Source,
    Disassembly,
    Registers,
    Pipeline,
    Terminal,
    Display,
    Bus,
    Events,
    Triggers,
    Faults,
    Waves,
    Hierarchy,
    Inspector,
    Layout,
    Transistors,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Scene => "3D",
            Tab::Source => "Source",
            Tab::Disassembly => "Disassembly",
            Tab::Registers => "Registers",
            Tab::Pipeline => "Pipeline",
            Tab::Terminal => "Terminal",
            Tab::Display => "Display",
            Tab::Bus => "Bus",
            Tab::Events => "Events",
            Tab::Triggers => "Triggers",
            Tab::Faults => "Faults",
            Tab::Waves => "Waves",
            Tab::Hierarchy => "Hierarchy",
            Tab::Inspector => "Inspector",
            Tab::Layout => "Layout",
            Tab::Transistors => "Transistors",
        }
    }
}

/// Data that never changes during a session.
pub struct Statics {
    pub compiled: Arc<Compiled>,
    pub mods: Vec<ModuleInfo>,
    pub stages: StageMap,
    pub source_files: Vec<PathBuf>,
    /// The placed and routed layout, when one was found.
    pub physical: Option<Arc<Physical>>,
}

pub struct App {
    pub worker: Worker,
    pub st: Arc<Statics>,
    pub view: View,
    dock: Option<DockState<Tab>>,
    pub sel: Selection,
    pub scene: SceneView,
    source_file: Option<PathBuf>,
    source_cache: BTreeMap<PathBuf, Vec<String>>,
    follow: bool,
    terminal_input: String,
    goto_text: String,
    trigger_text: String,
    trigger_kind: usize,
    fault_text: String,
    wave_signals: Vec<Signal>,
    wave_text: String,
    wave_window: u64,
    net_search: String,
    display_tex: Option<egui::TextureHandle>,
    display_seen: u64,
    prev_regs: [u32; 32],
    status_line: String,
    pub layout_view: LayoutView,
    pub transistors: TransistorView,
}

impl App {
    pub fn new(worker: Worker, st: Arc<Statics>, scene: SceneView) -> App {
        let mut dock = DockState::new(vec![Tab::Scene, Tab::Layout, Tab::Display, Tab::Waves]);
        let surface = dock.main_surface_mut();
        let [center, right] = surface.split_right(
            NodeIndex::root(),
            0.68,
            vec![Tab::Registers, Tab::Pipeline, Tab::Inspector, Tab::Transistors, Tab::Hierarchy],
        );
        let [center, _left] = surface.split_left(center, 0.30, vec![Tab::Source, Tab::Disassembly]);
        let _ = surface.split_below(center, 0.70, vec![Tab::Terminal, Tab::Bus, Tab::Events, Tab::Triggers, Tab::Faults]);
        let _ = right;
        App {
            worker,
            st,
            view: View::default(),
            dock: Some(dock),
            sel: Selection::default(),
            scene,
            source_file: None,
            source_cache: BTreeMap::new(),
            follow: true,
            terminal_input: String::new(),
            goto_text: String::new(),
            trigger_text: String::new(),
            trigger_kind: 0,
            fault_text: String::new(),
            wave_signals: Vec::new(),
            wave_text: String::new(),
            wave_window: 120,
            net_search: String::new(),
            display_tex: None,
            display_seen: u64::MAX,
            prev_regs: [0; 32],
            status_line: String::new(),
            layout_view: LayoutView::default(),
            transistors: TransistorView::new(transistor_view::start_loading()),
        }
    }

    /// Makes `tab` the visible tab of its group.
    pub fn focus_tab(&mut self, tab: Tab) {
        if let Some(dock) = self.dock.as_mut() {
            if let Some(loc) = dock.find_tab(&tab) {
                let _ = dock.set_active_tab(loc);
            }
        }
    }

    fn refresh(&mut self) {
        let prev = std::mem::take(&mut self.view);
        let mut s = self.worker.session.lock().unwrap();
        let v = View::gather(&mut s, &prev);
        drop(s);
        if prev.cycle == 0 && prev.stages.is_empty() {
            self.prev_regs = v.regs;
        } else if v.cycle != prev.cycle {
            self.prev_regs = prev.regs;
        }
        self.view = v;
    }

    fn send(&self, c: Command) {
        self.worker.send(c);
    }

    fn source(&mut self, path: &PathBuf) -> &[String] {
        self.source_cache
            .entry(path.clone())
            .or_insert_with(|| std::fs::read_to_string(path).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default())
    }

    fn parse_addr(&self, text: &str) -> Option<u32> {
        let t = text.trim();
        if let Some(h) = t.strip_prefix("0x") {
            return u32::from_str_radix(h, 16).ok();
        }
        if let Ok(v) = t.parse() {
            return Some(v);
        }
        let s = self.worker.session.lock().unwrap();
        s.debug.images.iter().find_map(|i| i.symbol(t).map(|s| s.addr))
    }

    fn line_breakpoint(&mut self, loc: Loc) {
        let s = self.worker.session.lock().unwrap();
        let mut pcs: Vec<u32> = s.debug.images.iter().flat_map(|i| i.addrs_of_line(&loc)).collect();
        drop(s);
        pcs.sort_unstable();
        pcs.dedup();
        if pcs.is_empty() {
            self.status_line = format!("no code for {}:{}", loc.file.display(), loc.line);
            return;
        }
        let label = format!("{}:{}", loc.file.file_name().unwrap_or_default().to_string_lossy(), loc.line);
        self.send(Command::AddTrigger(TriggerKind::CommitAny(pcs), label));
    }

    // ------------------------------------------------------------------ toolbar

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let status = self.worker.status.lock().unwrap().clone();
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Strata").strong());
            ui.separator();
            if status.running {
                if ui.button("Pause").clicked() {
                    self.send(Command::Pause);
                }
            } else if ui.button("Run").on_hover_text("run until a trigger fires or the machine halts").clicked() {
                self.send(Command::Run);
            }
            ui.add_enabled_ui(!status.running, |ui| {
                if ui.button("Cycle").on_hover_text("advance one clock cycle").clicked() {
                    self.send(Command::StepCycle);
                }
                if ui.button("Instruction").on_hover_text("run until the next instruction completes").clicked() {
                    self.send(Command::StepInstruction);
                }
                if ui.button("Line").on_hover_text("run until the source line changes").clicked() {
                    self.send(Command::StepLine);
                }
                if ui.button("Back").on_hover_text("go back to the previous instruction (replay)").clicked() {
                    self.send(Command::ReverseInstruction);
                }
                ui.separator();
                ui.label("cycle");
                let r = ui.add(egui::TextEdit::singleline(&mut self.goto_text).desired_width(90.0).hint_text("goto"));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Ok(c) = self.goto_text.trim().parse() {
                        self.send(Command::Goto(c));
                    }
                }
                if ui.button("Checkpoint").clicked() {
                    self.send(Command::Checkpoint);
                }
                if ui.button("Save session").on_hover_text("save the timeline for deterministic replay").clicked() {
                    self.send(Command::SaveSession(PathBuf::from("strata-session.json")));
                }
            });
            ui.separator();
            let v = &self.view;
            ui.label(RichText::new(format!("cycle {}", v.cycle)).monospace().strong());
            ui.label(RichText::new(format!("minstret {}", self.view.instret)).weak());
            if status.running {
                ui.label(format!("{:.0} cycles/s", status.speed));
            }
            if let Some(b) = &status.busy {
                ui.spinner();
                ui.label(b);
            } else if let Some(m) = &status.last_stop {
                ui.label(RichText::new(m).italics());
            }
            if !self.status_line.is_empty() {
                ui.label(RichText::new(&self.status_line).color(Color32::YELLOW));
            }
            if v.halted {
                ui.label(RichText::new(format!("HALTED ({})", v.halt_code)).color(Color32::LIGHT_RED).strong());
            }
            ui.label(RichText::new(format!("mode {}", if v.mode == 3 { "M" } else { "U" })).monospace());
            ui.label(RichText::new("gate level: sky130 netlist").weak()).on_hover_text(
                "Execution is determined by the synthesized netlist, evaluated cell by cell. \
                 Memories and board devices are the only abstractions (docs/abstractions.md).",
            );
        });
        // Timeline: checkpoints, events and the position; click to go there.
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click_and_drag());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 2.0, Color32::from_gray(28));
        let end = self.view.head.max(self.view.cycle).max(1) as f32;
        let x = |c: u64| rect.left() + rect.width() * (c as f32 / end);
        for &c in &self.view.checkpoints {
            p.line_segment([egui::pos2(x(c), rect.top()), egui::pos2(x(c), rect.top() + 5.0)], (1.0, Color32::from_gray(110)));
        }
        for (c, e) in &self.view.events {
            let col = match e {
                Event::Trap { interrupt: false, .. } => Color32::from_rgb(255, 120, 80),
                Event::Trap { .. } => Color32::from_rgb(120, 170, 255),
                Event::Trigger { .. } => Color32::YELLOW,
                Event::Halt { .. } => Color32::RED,
                Event::Fault { .. } => Color32::from_rgb(255, 60, 200),
                Event::Frame => continue,
            };
            p.line_segment([egui::pos2(x(*c), rect.top() + 6.0), egui::pos2(x(*c), rect.bottom())], (1.0, col));
        }
        p.line_segment([egui::pos2(x(self.view.cycle), rect.top()), egui::pos2(x(self.view.cycle), rect.bottom())], (2.0, Color32::WHITE));
        p.text(rect.right_center() - egui::vec2(4.0, 0.0), egui::Align2::RIGHT_CENTER, format!("{}", self.view.head), egui::FontId::monospace(10.0), Color32::GRAY);
        if resp.clicked() || resp.drag_stopped() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let c = ((pos.x - rect.left()) / rect.width() * end).max(0.0) as u64;
                self.send(Command::Goto(c));
            }
        }
        resp.on_hover_text("timeline: checkpoints (top ticks), traps, interrupts, triggers, faults; click to go to a cycle");
    }

    // ------------------------------------------------------------------ panels

    fn source_panel(&mut self, ui: &mut egui::Ui) {
        // Lines of the instructions in flight, by stage.
        let mut marks: BTreeMap<(PathBuf, u32), Vec<usize>> = BTreeMap::new();
        for (i, s) in self.view.stages.iter().enumerate() {
            if let (true, Some(l)) = (s.valid, &s.source) {
                marks.entry((l.file.clone(), l.line)).or_default().push(i);
            }
        }
        let current = self.view.stages.iter().rev().find(|s| s.valid).and_then(|s| s.source.clone());
        if self.follow {
            if let Some(l) = &current {
                self.source_file = Some(l.file.clone());
            }
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.follow, "follow");
            let label = self.source_file.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "choose a file".into());
            egui::ComboBox::from_id_salt("srcfile").selected_text(label).width(ui.available_width() - 10.0).show_ui(ui, |ui| {
                for f in self.st.source_files.clone() {
                    let t = f.display().to_string();
                    if ui.selectable_label(self.source_file.as_ref() == Some(&f), t).clicked() {
                        self.source_file = Some(f);
                        self.follow = false;
                    }
                }
            });
        });
        let Some(file) = self.source_file.clone() else { return };
        let bps: Vec<String> = self.view.triggers.iter().map(|t| t.label.clone()).collect();
        let fname = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let lines = self.source(&file).to_vec();
        let target_line = current.as_ref().filter(|l| l.file == file).map(|l| l.line);
        let mut clicked_line = None;
        egui::ScrollArea::both().auto_shrink(false).show_rows(ui, 16.0, lines.len(), |ui, range| {
            for i in range {
                let n = i as u32 + 1;
                ui.horizontal(|ui| {
                    let bp = bps.contains(&format!("{fname}:{n}"));
                    let gutter = ui.add(egui::Label::new(RichText::new(if bp { "●" } else { " " }).color(Color32::RED).monospace()).sense(egui::Sense::click()));
                    if gutter.clicked() {
                        clicked_line = Some(n);
                    }
                    let num = ui.add(egui::Label::new(RichText::new(format!("{n:5}")).monospace().color(Color32::GRAY)).sense(egui::Sense::click()));
                    if num.clicked() {
                        clicked_line = Some(n);
                    }
                    let st = marks.get(&(file.clone(), n));
                    let tag = st.map(|v| v.iter().map(|&k| inspect::STAGES[k]).collect::<String>()).unwrap_or_default();
                    let col = st.map(|v| {
                        let c = STAGE_COLORS[*v.last().unwrap()];
                        Color32::from_rgb(c[0], c[1], c[2])
                    });
                    ui.label(RichText::new(format!("{tag:>3}")).monospace().color(col.unwrap_or(Color32::GRAY)));
                    let mut text = RichText::new(&lines[i]).monospace();
                    if let Some(c) = col {
                        text = text.color(c);
                    }
                    let r = ui.label(text);
                    if Some(n) == target_line && self.follow {
                        r.scroll_to_me(Some(egui::Align::Center));
                    }
                });
            }
        });
        if let Some(n) = clicked_line {
            let label = format!("{fname}:{n}");
            if let Some(t) = self.view.triggers.iter().find(|t| t.label == label) {
                self.send(Command::RemoveTrigger(t.id));
            } else {
                self.line_breakpoint(Loc { file: file.clone(), line: n });
            }
        }
    }

    fn disasm_panel(&mut self, ui: &mut egui::Ui) {
        let center = self.sel.pc.or_else(|| self.view.stages.iter().rev().find(|s| s.valid).map(|s| s.pc)).unwrap_or(0) & !3;
        let s = self.worker.session.lock().unwrap();
        let mut rows = Vec::new();
        for k in -24i64..40 {
            let pc = (center as i64 + k * 4) as u32;
            let Some(w) = inspect::read_word(&s, pc) else { continue };
            let label = inspect::symbolize(&s, pc).filter(|n| !n.contains('+'));
            rows.push((pc, w, inspect::disassemble(&s, pc, w), label));
        }
        drop(s);
        let stage_of: BTreeMap<u32, Vec<usize>> = self.view.stages.iter().enumerate().filter(|(_, s)| s.valid).fold(BTreeMap::new(), |mut m, (i, s)| {
            m.entry(s.pc).or_default().push(i);
            m
        });
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (pc, w, text, label) in rows {
                if let Some(l) = label {
                    ui.label(RichText::new(format!("{l}:")).monospace().strong());
                }
                ui.horizontal(|ui| {
                    let tag: String = stage_of.get(&pc).map(|v| v.iter().map(|&k| inspect::STAGES[k]).collect()).unwrap_or_default();
                    let col = stage_of.get(&pc).map(|v| {
                        let c = STAGE_COLORS[*v.last().unwrap()];
                        Color32::from_rgb(c[0], c[1], c[2])
                    });
                    ui.label(RichText::new(format!("{tag:>3}")).monospace().color(col.unwrap_or(Color32::GRAY)));
                    let bp = self.view.triggers.iter().any(|t| t.kind == TriggerKind::Commit(pc));
                    let a = ui.add(egui::Label::new(RichText::new(format!("{}{pc:08x}", if bp { "●" } else { " " })).monospace().color(if bp { Color32::RED } else { Color32::GRAY })).sense(egui::Sense::click()));
                    if a.clicked() {
                        match self.view.triggers.iter().find(|t| t.kind == TriggerKind::Commit(pc)) {
                            Some(t) => self.send(Command::RemoveTrigger(t.id)),
                            None => self.send(Command::AddTrigger(TriggerKind::Commit(pc), format!("{pc:08x}"))),
                        }
                    }
                    ui.label(RichText::new(format!("{w:08x}")).monospace().weak());
                    let mut t = RichText::new(text).monospace();
                    if let Some(c) = col {
                        t = t.color(c);
                    }
                    if ui.add(egui::Label::new(t).sense(egui::Sense::click())).clicked() {
                        self.sel.pc = Some(pc);
                    }
                });
            }
        });
    }

    fn registers_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.label(RichText::new("Integer registers (read from the register file's flip-flops)").weak());
            egui::Grid::new("regs").striped(true).show(ui, |ui| {
                for i in 0..32 {
                    let v = self.view.regs[i];
                    let changed = v != self.prev_regs[i];
                    ui.label(RichText::new(format!("x{i:<2} {:>4}", strata_isa::reg_name(i as u8))).monospace());
                    let t = RichText::new(format!("{v:08x}")).monospace();
                    ui.label(if changed { t.color(Color32::YELLOW).strong() } else { t });
                    ui.label(RichText::new(format!("{}", v as i32)).monospace().weak());
                    ui.end_row();
                }
            });
            ui.separator();
            ui.label(RichText::new("Control and status registers").weak());
            egui::Grid::new("csrs").striped(true).show(ui, |ui| {
                ui.label(RichText::new("privilege").monospace());
                ui.label(RichText::new(if self.view.mode == 3 { "M" } else { "U" }).monospace());
                ui.end_row();
                for (name, v) in &self.view.csrs {
                    ui.label(RichText::new(*name).monospace());
                    ui.label(RichText::new(format!("{v:#x}")).monospace());
                    ui.end_row();
                }
            });
        });
    }

    fn pipeline_panel(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("stages").striped(true).show(ui, |ui| {
            for s in &self.view.stages.clone() {
                let i = inspect::STAGES.iter().position(|x| *x == s.name).unwrap_or(0);
                let c = STAGE_COLORS[i];
                ui.label(RichText::new(s.name).strong().color(Color32::from_rgb(c[0], c[1], c[2])));
                if s.valid {
                    if ui.link(RichText::new(format!("{:08x}", s.pc)).monospace()).clicked() {
                        self.sel.pc = Some(s.pc);
                    }
                    match s.insn {
                        Some(w) => ui.label(RichText::new(format!("{w:08x}")).monospace()).on_hover_text("instruction word in this stage's pipeline register"),
                        None => ui.label(RichText::new("--------").monospace().weak()).on_hover_text("this stage keeps only the PC; the text is decoded from memory"),
                    };
                    ui.label(RichText::new(&s.text).monospace());
                    ui.label(RichText::new(s.symbol.clone().unwrap_or_default()).weak());
                } else {
                    ui.label(RichText::new("bubble").weak());
                }
                ui.end_row();
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (name, on) in &self.view.control {
                let col = if *on { Color32::YELLOW } else { Color32::from_gray(90) };
                ui.label(RichText::new(format!("● {name}")).color(col));
            }
        });
        ui.separator();
        ui.label(RichText::new("Completed instructions (newest first)").weak());
        let retired: Vec<_> = self.view.retired.iter().rev().take(200).cloned().collect();
        let s = self.worker.session.lock().unwrap();
        let rows: Vec<_> = retired.iter().map(|r| (*r, inspect::disassemble(&s, r.pc, r.insn), inspect::symbolize(&s, r.pc))).collect();
        drop(s);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (r, text, sym) in rows {
                ui.horizontal(|ui| {
                    if ui.link(RichText::new(format!("{:>9}", r.cycle)).monospace().weak()).on_hover_text("go to this cycle").clicked() {
                        self.send(Command::Goto(r.cycle));
                    }
                    ui.label(RichText::new(format!("{:08x}", r.pc)).monospace());
                    ui.label(RichText::new(format!("{text:<32}")).monospace());
                    if r.trap {
                        ui.label(RichText::new(format!("trap {}{}", r.cause, if r.intr { " (interrupt)" } else { "" })).color(Color32::LIGHT_RED));
                    } else if let Some((rd, v)) = r.rd {
                        ui.label(RichText::new(format!("{} = {v:08x}", strata_isa::reg_name(rd))).monospace().color(Color32::from_rgb(160, 210, 160)));
                    }
                    ui.label(RichText::new(sym.unwrap_or_default()).weak());
                });
            }
        });
    }

    fn terminal_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("buttons:");
            let names = ["UP", "DOWN", "LEFT", "RIGHT", "A", "B", "START"];
            let mut b = self.view.buttons;
            for (k, n) in names.iter().enumerate() {
                let mut on = b & (1 << k) != 0;
                if ui.toggle_value(&mut on, *n).changed() {
                    b ^= 1 << k;
                    self.send(Command::Record(TimelineEvent::Input(InputEvent::Buttons(b))));
                }
            }
            ui.separator();
            ui.label("LEDs:");
            for k in (0..8).rev() {
                let on = self.view.leds & (1 << k) != 0;
                ui.label(RichText::new("●").color(if on { Color32::from_rgb(80, 255, 90) } else { Color32::from_gray(50) }));
            }
        });
        let input = ui.horizontal(|ui| {
            ui.label("type:");
            let r = ui.add(egui::TextEdit::singleline(&mut self.terminal_input).desired_width(ui.available_width() - 60.0).hint_text("text sent to the UART; Enter adds a newline"));
            let sent = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            sent || ui.button("send").clicked()
        });
        if input.inner {
            let mut bytes = self.terminal_input.clone().into_bytes();
            bytes.push(b'\n');
            self.send(Command::Record(TimelineEvent::Input(InputEvent::Uart(bytes))));
            self.terminal_input.clear();
        }
        let text = strata_sim::script::strip_ansi(&self.view.uart).replace('\r', "");
        egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            ui.label(RichText::new(text).monospace());
        });
    }

    fn display_panel(&mut self, ui: &mut egui::Ui) {
        if let Some(d) = &self.view.display {
            if self.display_seen != self.view.display_frames || self.display_tex.is_none() {
                let img = egui::ColorImage::from_rgb([640, 480], d);
                match &mut self.display_tex {
                    Some(t) => t.set(img, egui::TextureOptions::NEAREST),
                    None => self.display_tex = Some(ui.ctx().load_texture("display", img, egui::TextureOptions::NEAREST)),
                }
                self.display_seen = self.view.display_frames;
            }
        }
        ui.label(RichText::new(format!(
            "Pixels captured from the video pins (vid_r/g/b, syncs); frame {}. At the gate level a frame takes 420,000 cycles.",
            self.view.display_frames
        )).weak());
        if let Some(t) = &self.display_tex {
            let avail = ui.available_size();
            let scale = (avail.x / 640.0).min(avail.y / 480.0).max(0.25);
            ui.image((t.id(), egui::vec2(640.0 * scale, 480.0 * scale)));
        }
    }

    fn bus_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Data-bus transactions at the CPU's bus port, newest first").weak());
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("bus").striped(true).show(ui, |ui| {
                for h in ["cycle", "", "address", "region", "data", "response"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for b in self.view.bus.iter().rev().take(300) {
                    if ui.link(RichText::new(format!("{}", b.cycle)).monospace()).clicked() {
                        self.send(Command::Goto(b.cycle));
                    }
                    ui.label(if b.write { "W" } else { "R" });
                    ui.label(RichText::new(format!("{:08x}", b.addr)).monospace());
                    ui.label(inspect::region_of(b.addr));
                    let data = match (b.write, b.response) {
                        (true, _) => format!("{:08x} be={:04b}", b.wdata, b.be),
                        (false, Some((_, d, _))) => format!("{d:08x}"),
                        _ => "…".into(),
                    };
                    ui.label(RichText::new(data).monospace());
                    ui.label(match b.response {
                        Some((c, _, true)) => RichText::new(format!("error (+{})", c - b.cycle)).color(Color32::LIGHT_RED),
                        Some((c, _, false)) => RichText::new(format!("+{}", c - b.cycle)),
                        None => RichText::new("pending").weak(),
                    });
                    ui.end_row();
                }
            });
        });
    }

    fn events_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (c, e) in self.view.events.iter().rev().filter(|(_, e)| !matches!(e, Event::Frame)).take(500) {
                ui.horizontal(|ui| {
                    if ui.link(RichText::new(format!("{c:>10}")).monospace()).clicked() {
                        self.send(Command::Goto(*c));
                    }
                    ui.label(match e {
                        Event::Trap { pc, cause, interrupt } => format!("{} {cause} at {pc:08x}", if *interrupt { "interrupt" } else { "exception" }),
                        Event::Trigger { id } => format!("trigger {id}"),
                        Event::Halt { code } => format!("halt {code}"),
                        Event::Fault { description } => format!("fault: {description}"),
                        Event::Frame => String::new(),
                    });
                });
            }
        });
    }

    fn triggers_panel(&mut self, ui: &mut egui::Ui) {
        const KINDS: [&str; 7] = ["address or symbol", "memory access (addr or lo..hi)", "signal == value", "signal changes", "trap", "terminal text", "cycle"];
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("tk").selected_text(KINDS[self.trigger_kind]).show_ui(ui, |ui| {
                for (i, k) in KINDS.iter().enumerate() {
                    ui.selectable_value(&mut self.trigger_kind, i, *k);
                }
            });
            let label = ui.label("stop at:");
            ui.add(egui::TextEdit::singleline(&mut self.trigger_text).desired_width(220.0).hint_text(match self.trigger_kind {
                0 => "main, 0x10000044",
                1 => "0x20000000..0x20002000",
                2 => "u_core.m_valid 1",
                3 => "u_core.u_csr.mode_q",
                4 => "cause number, or empty for any",
                5 => "text",
                _ => "cycle",
            }))
            .labelled_by(label.id);
            if ui.button("Add").clicked() {
                self.add_trigger();
            }
        });
        ui.label(RichText::new("A trigger stops a run at the exact cycle it fires. Address triggers stop with the instruction in W, before its effects.").weak());
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for t in self.view.triggers.clone() {
                ui.horizontal(|ui| {
                    let mut en = t.enabled;
                    if ui.checkbox(&mut en, "").changed() {
                        self.send(Command::ToggleTrigger(t.id));
                    }
                    ui.label(RichText::new(format!("#{} {}", t.id, t.label)).monospace());
                    ui.label(RichText::new(format!("{} hits", t.hits)).weak());
                    if ui.small_button("remove").clicked() {
                        self.send(Command::RemoveTrigger(t.id));
                    }
                });
            }
        });
    }

    fn resolve_signal(&self, name: &str) -> Option<Signal> {
        let idx = self.st.compiled.netlist.name_index();
        strata_debug::probes::resolve(&idx, name.trim())
    }

    fn add_trigger(&mut self) {
        let text = self.trigger_text.trim().to_string();
        let kind = match self.trigger_kind {
            0 => self.parse_addr(&text).map(TriggerKind::Commit),
            1 => {
                let (lo, hi) = match text.split_once("..") {
                    Some((a, b)) => (self.parse_addr(a), self.parse_addr(b)),
                    None => (self.parse_addr(&text), self.parse_addr(&text).map(|a| a + 4)),
                };
                lo.zip(hi).map(|(lo, hi)| TriggerKind::Access { lo, hi, read: true, write: true })
            }
            2 => {
                let mut it = text.split_whitespace();
                let sig = it.next().and_then(|n| self.resolve_signal(n));
                let val = it.next().and_then(|v| self.parse_addr(v)).unwrap_or(1) as u64;
                sig.map(|signal| TriggerKind::Signal { signal, op: CmpOp::Eq, value: val })
            }
            3 => self.resolve_signal(&text).map(|signal| TriggerKind::Change { signal }),
            4 => Some(TriggerKind::Trap { cause: text.parse().ok(), interrupt: None }),
            5 => (!text.is_empty()).then(|| TriggerKind::Uart(text.clone())),
            _ => text.parse().ok().map(TriggerKind::Cycle),
        };
        match kind {
            Some(k) => {
                self.send(Command::AddTrigger(k, text));
                self.trigger_text.clear();
            }
            None => self.status_line = format!("could not understand trigger '{text}'"),
        }
    }

    fn faults_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Faults are recorded in the timeline at the current cycle and replay deterministically.").weak());
        let cell = self.sel.cell;
        let flop = cell.and_then(|c| self.st.compiled.flop_of_cell(c));
        ui.horizontal(|ui| {
            ui.add_enabled_ui(flop.is_some(), |ui| {
                if ui.button("Upset selected flip-flop").clicked() {
                    self.send(Command::Record(TimelineEvent::Fault(FaultAction::Flip { flop: flop.unwrap() })));
                }
            });
            let net = self.sel.net;
            ui.add_enabled_ui(net.is_some(), |ui| {
                for (label, v) in [("Stuck at 0", Some(false)), ("Stuck at 1", Some(true)), ("Release", None)] {
                    if ui.button(label).clicked() {
                        self.send(Command::Record(TimelineEvent::Fault(FaultAction::Force { net: net.unwrap(), value: v })));
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("memory bit flip:");
            ui.add(egui::TextEdit::singleline(&mut self.fault_text).desired_width(160.0).hint_text("address bit, e.g. 0x10000100 3"));
            if ui.button("Flip").clicked() {
                let mut it = self.fault_text.split_whitespace();
                let addr = it.next().and_then(|a| self.parse_addr(a));
                let bit: Option<u8> = it.next().and_then(|b| b.parse().ok());
                if let (Some(a), Some(b)) = (addr, bit) {
                    let (mem, base) = match inspect::region_of(a) {
                        "ROM" => (Some(MemId::Rom), 0),
                        "RAM" => (Some(MemId::Ram), 0x1000_0000),
                        "VRAM" => (Some(MemId::Vram), 0x2000_0000),
                        "FONT" => (Some(MemId::Font), 0x2001_0000),
                        _ => (None, 0),
                    };
                    if let Some(mem) = mem {
                        self.send(Command::Record(TimelineEvent::Fault(FaultAction::MemFlip { mem, word: (a - base) / 4, bit: b })));
                    }
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let s = self.worker.session.lock().unwrap();
            let rows: Vec<(u64, String)> = self
                .view
                .timeline
                .iter()
                .map(|(c, e)| match e {
                    TimelineEvent::Fault(f) => (*c, s.describe_fault(f)),
                    TimelineEvent::Input(InputEvent::Uart(b)) => (*c, format!("typed {:?}", String::from_utf8_lossy(b))),
                    TimelineEvent::Input(InputEvent::Buttons(b)) => (*c, format!("buttons {b:07b}")),
                    TimelineEvent::Input(InputEvent::Reset(n)) => (*c, format!("reset for {n} cycles")),
                })
                .collect();
            drop(s);
            ui.label(RichText::new("Timeline (inputs and faults)").strong());
            for (c, d) in rows {
                ui.horizontal(|ui| {
                    if ui.link(RichText::new(format!("{c:>10}")).monospace()).clicked() {
                        self.send(Command::Goto(c));
                    }
                    ui.label(d);
                });
            }
        });
    }

    fn hierarchy_panel(&mut self, ui: &mut egui::Ui) {
        let st = Arc::clone(&self.st);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            self.module_node(ui, &st, 0);
        });
    }

    fn module_node(&mut self, ui: &mut egui::Ui, st: &Statics, i: u32) {
        let m = &st.mods[i as usize];
        let label = format!(
            "{}  ({})  {} cells, {} flops, {:.0} µm²",
            m.path.rsplit('.').next().unwrap_or(&m.path),
            m.module,
            m.total_cells,
            m.total_flops,
            m.total_area
        );
        let selected = self.sel.module == Some(i);
        let id = ui.make_persistent_id(("mod", i));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, i == 0)
            .show_header(ui, |ui| {
                if ui.selectable_label(selected, RichText::new(label).monospace()).clicked() {
                    self.sel.module = Some(i);
                    self.sel.cell = None;
                    self.scene.focus_module(i);
                }
            })
            .body(|ui| {
                for &c in &m.children {
                    self.module_node(ui, st, c);
                }
            });
    }

    fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let label = ui.label("find:");
            let r = ui.add(egui::TextEdit::singleline(&mut self.net_search).hint_text("cell u_core._c713_ or net u_core.x_pc[3]")).labelled_by(label.id);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let name = self.net_search.trim().to_string();
                if let Some(&cell) = inspect::cell_index(&self.st.compiled).get(name.as_str()) {
                    self.sel.select_cell(&self.st.compiled, cell);
                } else if let Some(n) = self.st.compiled.netlist.net_by_name(&name) {
                    self.sel.net = Some(n);
                    self.sel.cell = inspect::driver_cell(&self.st.compiled, n);
                } else {
                    self.status_line = format!("no cell or net named {name}");
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if let Some(cell) = self.sel.cell {
                let s = self.worker.session.lock().unwrap();
                let info = inspect::cell_info(&s, cell);
                drop(s);
                ui.label(RichText::new(&info.name).monospace().strong());
                egui::Grid::new("cell").show(ui, |ui| {
                    ui.label("type");
                    ui.label(RichText::new(&info.cell_type).monospace());
                    ui.end_row();
                    ui.label("module");
                    ui.label(RichText::new(&info.module_path).monospace());
                    ui.end_row();
                    ui.label("area");
                    ui.label(format!("{:.2} µm²", info.area));
                    ui.end_row();
                    let stages = self.st.stages.stages[cell as usize];
                    ui.label("stages");
                    ui.label((0..5).filter(|k| stages & (1 << k) != 0).map(|k| inspect::STAGES[k]).collect::<Vec<_>>().join(" "));
                    ui.end_row();
                    for (o, f) in &info.functions {
                        ui.label(format!("{o} ="));
                        ui.label(RichText::new(f).monospace());
                        ui.end_row();
                    }
                    if let Some(st) = info.state {
                        ui.label("stored bit");
                        ui.label(RichText::new(format!("{}", st as u8)).monospace().strong());
                        ui.end_row();
                    }
                });
                if let Some(src) = &info.src {
                    ui.label(RichText::new(format!("RTL: {src}")).monospace());
                    if let Some((file, line)) = parse_src(src) {
                        let path = PathBuf::from(&file);
                        let lines = self.source(&path).to_vec();
                        let lo = line.saturating_sub(3);
                        for n in lo..(line + 3).min(lines.len() as u32 + 1) {
                            if n == 0 {
                                continue;
                            }
                            let t = RichText::new(format!("{n:5} {}", lines[(n - 1) as usize])).monospace();
                            ui.label(if n == line { t.color(Color32::YELLOW) } else { t.weak() });
                        }
                    }
                }
                ui.separator();
                ui.label(RichText::new("Pins").strong());
                let mut goto_net = None;
                egui::Grid::new("pins").striped(true).show(ui, |ui| {
                    for p in &info.pins {
                        ui.label(RichText::new(format!("{} {}", if p.output { "out" } else { "in " }, p.pin)).monospace());
                        ui.label(RichText::new(format!("{}", p.value as u8)).monospace().strong().color(if p.value { Color32::from_rgb(250, 220, 90) } else { Color32::GRAY }));
                        if ui.link(RichText::new(&p.net_name).monospace()).clicked() {
                            goto_net = Some(p.net);
                        }
                        ui.end_row();
                    }
                });
                if let Some(n) = goto_net {
                    self.sel.net = Some(n);
                }
                ui.horizontal(|ui| {
                    if let Some(n) = self.sel.net {
                        if ui.button("add net to waves").clicked() {
                            let name = self.st.compiled.netlist.net_name(n);
                            self.wave_signals.push(Signal { name, bits: vec![n], lsb: 0 });
                        }
                        if let Some(d) = inspect::driver_cell(&self.st.compiled, n) {
                            if ui.button("go to its driver").clicked() {
                                self.sel.select_cell(&self.st.compiled, d);
                            }
                        }
                    }
                });
            } else if let Some(m) = self.sel.module {
                let m = &self.st.mods[m as usize];
                ui.label(RichText::new(&m.path).monospace().strong());
                ui.label(format!("module {}: {} cells ({} own), {} flip-flops, {:.0} µm²", m.module, m.total_cells, m.cells.len(), m.total_flops, m.total_area));
            } else {
                ui.label("Select a cell in the 3D view or the layout, a module in the hierarchy, or type a cell or net name above and press Enter.");
            }
        });
    }

    fn waves_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("signal:");
            let r = ui.add(egui::TextEdit::singleline(&mut self.wave_text).desired_width(200.0).hint_text("u_core.w_pc"));
            let add = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || ui.button("add").clicked();
            if add {
                match self.resolve_signal(&self.wave_text) {
                    Some(s) => {
                        self.wave_signals.push(s);
                        self.wave_text.clear();
                    }
                    None => self.status_line = format!("no signal named {}", self.wave_text.trim()),
                }
            }
            if ui.button("pipeline preset").clicked() {
                for n in ["u_core.x_valid", "u_core.x_pc", "u_core.m_valid", "u_core.m_pc", "u_core.w_valid", "u_core.w_pc", "u_core.x_hazard", "u_core.w_flush", "u_core.dbus_cmd_valid", "u_core.dbus_cmd_addr"] {
                    if let Some(s) = self.resolve_signal(n) {
                        self.wave_signals.push(s);
                    }
                }
            }
            ui.add(egui::Slider::new(&mut self.wave_window, 16..=2000).text("cycles").logarithmic(true));
            if ui.button("capture").on_hover_text("replays the window before the current cycle").clicked() && !self.wave_signals.is_empty() {
                let end = self.view.cycle + 1;
                let start = end.saturating_sub(self.wave_window);
                self.send(Command::Waves { signals: self.wave_signals.clone(), start, end });
            }
            if ui.button("clear").clicked() {
                self.wave_signals.clear();
            }
        });
        let waves = self.worker.status.lock().unwrap().waves.clone();
        let Some(w) = waves else {
            ui.label("Add signals and press capture. Waveforms are rebuilt by deterministic replay from the nearest checkpoint.");
            return;
        };
        if ui.button("export VCD").clicked() {
            if let Ok(mut f) = std::fs::File::create("strata-waves.vcd") {
                let _ = w.write_vcd(&mut f);
                self.status_line = "wrote strata-waves.vcd".into();
            }
        }
        let rows = w.signals.len();
        let name_w = 190.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), (rows as f32 * 22.0 + 20.0).max(40.0)), egui::Sense::click());
        let p = ui.painter_at(rect);
        let n = w.values.len().max(1);
        let dx = (rect.width() - name_w) / n as f32;
        for (k, sig) in w.signals.iter().enumerate() {
            let y = rect.top() + k as f32 * 22.0 + 4.0;
            p.text(egui::pos2(rect.left(), y + 8.0), egui::Align2::LEFT_CENTER, &sig.name, egui::FontId::monospace(11.0), Color32::LIGHT_GRAY);
            let mut prev: Option<u64> = None;
            for (t, row) in w.values.iter().enumerate() {
                let x0 = rect.left() + name_w + t as f32 * dx;
                let v = row[k];
                if sig.width() == 1 {
                    let yv = if v != 0 { y } else { y + 14.0 };
                    p.line_segment([egui::pos2(x0, yv), egui::pos2(x0 + dx, yv)], (1.5, Color32::from_rgb(120, 230, 120)));
                    if let Some(pv) = prev {
                        if pv != v {
                            p.line_segment([egui::pos2(x0, y), egui::pos2(x0, y + 14.0)], (1.0, Color32::from_rgb(120, 230, 120)));
                        }
                    }
                } else {
                    let changed = prev != Some(v);
                    p.line_segment([egui::pos2(x0, y), egui::pos2(x0 + dx, y)], (1.0, Color32::from_rgb(120, 180, 255)));
                    p.line_segment([egui::pos2(x0, y + 14.0), egui::pos2(x0 + dx, y + 14.0)], (1.0, Color32::from_rgb(120, 180, 255)));
                    if changed {
                        p.line_segment([egui::pos2(x0, y), egui::pos2(x0, y + 14.0)], (1.0, Color32::from_rgb(120, 180, 255)));
                        if dx > 24.0 || t % ((30.0 / dx.max(0.1)) as usize).max(1) == 0 {
                            p.text(egui::pos2(x0 + 2.0, y + 7.0), egui::Align2::LEFT_CENTER, format!("{v:x}"), egui::FontId::monospace(9.0), Color32::WHITE);
                        }
                    }
                }
                prev = Some(v);
            }
        }
        // Cycle ruler and the current cycle.
        let cur = self.view.cycle.saturating_sub(w.start) as f32;
        let xc = rect.left() + name_w + cur * dx;
        p.line_segment([egui::pos2(xc, rect.top()), egui::pos2(xc, rect.bottom())], (1.0, Color32::YELLOW));
        p.text(egui::pos2(rect.left() + name_w, rect.bottom() - 8.0), egui::Align2::LEFT_CENTER, format!("{}", w.start), egui::FontId::monospace(9.0), Color32::GRAY);
        p.text(egui::pos2(rect.right(), rect.bottom() - 8.0), egui::Align2::RIGHT_CENTER, format!("{}", w.start + n as u64), egui::FontId::monospace(9.0), Color32::GRAY);
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if pos.x > rect.left() + name_w {
                    let c = w.start + ((pos.x - rect.left() - name_w) / dx) as u64;
                    self.send(Command::Goto(c));
                }
            }
        }
    }

    pub fn frame(&mut self, ui: &mut egui::Ui, rs: Option<&egui_wgpu::RenderState>) {
        self.refresh();
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        let mut dock = self.dock.take().unwrap();
        DockArea::new(&mut dock)
            .show_close_buttons(false)
            .show_inside(ui, &mut Viewer { app: self, rs });
        self.dock = Some(dock);
        if self.worker.status.lock().unwrap().running {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
}

/// Splits a Yosys source attribute ("rtl/core/st_alu.sv:27.5-27.40").
fn parse_src(src: &str) -> Option<(String, u32)> {
    let first = src.split('|').next()?;
    let (file, rest) = first.rsplit_once(':')?;
    let line = rest.split('.').next()?.parse().ok()?;
    Some((file.to_string(), line))
}

struct Viewer<'a> {
    app: &'a mut App,
    rs: Option<&'a egui_wgpu::RenderState>,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let a = &mut *self.app;
        match tab {
            Tab::Scene => {
                let st = Arc::clone(&a.st);
                a.scene.window = a.worker.status.lock().unwrap().activity.clone();
                a.scene.ui(ui, self.rs, &st.compiled, &st.mods, &st.stages, &a.view, &mut a.sel, st.physical.as_deref());
                if let Some((start, end)) = a.scene.request_window.take() {
                    a.send(Command::Activity { start, end });
                }
            }
            Tab::Source => a.source_panel(ui),
            Tab::Disassembly => a.disasm_panel(ui),
            Tab::Registers => a.registers_panel(ui),
            Tab::Pipeline => a.pipeline_panel(ui),
            Tab::Terminal => a.terminal_panel(ui),
            Tab::Display => a.display_panel(ui),
            Tab::Bus => a.bus_panel(ui),
            Tab::Events => a.events_panel(ui),
            Tab::Triggers => a.triggers_panel(ui),
            Tab::Faults => a.faults_panel(ui),
            Tab::Waves => a.waves_panel(ui),
            Tab::Hierarchy => a.hierarchy_panel(ui),
            Tab::Inspector => a.inspector_panel(ui),
            Tab::Layout => {
                let st = Arc::clone(&a.st);
                a.layout_view.ui(ui, &st, &mut a.scene, &a.view, &mut a.sel);
            }
            Tab::Transistors => {
                let st = Arc::clone(&a.st);
                a.transistors.ui(ui, &st, &a.view, &a.sel);
            }
        }
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            Tab::Scene | Tab::Display | Tab::Waves => [false, false],
            _ => [false, false],
        }
    }
}
