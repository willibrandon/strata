//! The 3D view of the inspection scene (layout in scene.rs, drawing in
//! render3d.rs): camera, coloring, picking, labels.

use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Sense, Stroke};
use eframe::egui_wgpu;
use glam::{Mat4, Vec2, Vec3, Vec4};
use strata_debug::inspect::{self, ModuleInfo, StageMap};
use strata_gate::Compiled;
use strata_netlist::PortDir;

use crate::physical::Physical;
use crate::render3d::{BoxInstance, LineVertex, Renderer3d};
use crate::scene::{Placement, Rect, SceneLayout, SOURCE_HEIGHT, STAGE_HEIGHT};
use crate::view::View;
use crate::Selection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorBy {
    Module,
    Stage,
    Activity,
    Value,
}

pub const STAGE_COLORS: [[u8; 3]; 5] = [[90, 150, 255], [80, 210, 230], [110, 220, 110], [255, 170, 60], [230, 110, 220]];

/// Colors of the routing layers, bottom first (li1, met1 ... met5).
pub const LAYER_COLORS: [[u8; 3]; 6] = [[170, 120, 230], [80, 140, 255], [240, 90, 170], [90, 220, 170], [240, 220, 80], [255, 150, 60]];

/// Height of routing layer `l` in the physical view: above the (lowered)
/// cell boxes, one step per layer.
fn layer_y(extent: f32, l: u8) -> f32 {
    extent * (0.03 + 0.006 * l as f32)
}

struct Camera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Camera {
    fn eye(&self) -> Vec3 {
        let dir = Vec3::new(self.yaw.cos() * self.pitch.cos(), self.pitch.sin(), self.yaw.sin() * self.pitch.cos());
        self.target + dir * self.distance
    }

    fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y);
        let proj = glam::camera::rh::proj::directx::perspective(
            40f32.to_radians(),
            aspect.max(0.1),
            self.distance * 0.02,
            self.distance * 8.0,
        );
        proj * view
    }
}

pub struct SceneView {
    pub treemap: SceneLayout,
    pub physical: Option<SceneLayout>,
    pub use_physical: bool,
    renderer: Option<Renderer3d>,
    camera: Camera,
    prev_toggles: Option<(u64, Arc<Vec<u32>>)>,
    /// Smoothed toggles per cycle for each cell.
    activity: Vec<f32>,
    pub color_by: ColorBy,
    pub show_pipeline: bool,
    pub show_source: bool,
    pub show_nets: bool,
    module_hue: Vec<f32>,
    /// Toggle counts measured over a window of cycles (by replay); when
    /// present, activity coloring uses them instead of the live estimate.
    pub window: Option<Arc<inspect::Activity>>,
    pub window_len: u64,
    /// A window the user asked to measure: (start, end).
    pub request_window: Option<(u64, u64)>,
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

fn rgb(c: [u8; 3], a: f32) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, a]
}

impl SceneView {
    pub fn new(c: &Compiled, mods: &[ModuleInfo], stages: &StageMap) -> SceneView {
        let treemap = SceneLayout::treemap(c, mods, stages);
        let extent = treemap.extent.w();
        // A distinct hue for each leaf module instance.
        let leaves: Vec<usize> = (0..mods.len()).filter(|&i| !mods[i].cells.is_empty()).collect();
        let mut module_hue = vec![0.0; mods.len()];
        for (k, &i) in leaves.iter().enumerate() {
            module_hue[i] = (k as f32 * 0.618_034) % 1.0;
        }
        SceneView {
            treemap,
            physical: None,
            use_physical: false,
            renderer: None,
            camera: Camera { target: Vec3::new(0.0, extent * 0.3, 0.0), yaw: 0.95, pitch: 0.42, distance: extent * 2.1 },
            prev_toggles: None,
            activity: vec![0.0; c.netlist.cells.len()],
            color_by: ColorBy::Stage,
            show_pipeline: true,
            show_source: true,
            show_nets: true,
            module_hue,
            window: None,
            window_len: 20_000,
            request_window: None,
        }
    }

    pub fn layout(&self) -> &SceneLayout {
        match (&self.physical, self.use_physical) {
            (Some(p), true) => p,
            _ => &self.treemap,
        }
    }

    fn update_activity(&mut self, c: &Compiled, view: &View) {
        let Some(t) = &view.toggles else { return };
        if let Some((cycle, prev)) = &self.prev_toggles {
            let dc = view.cycle.saturating_sub(*cycle);
            if dc > 0 && prev.len() == t.len() {
                for (cell, a) in self.activity.iter_mut().enumerate() {
                    let mut n = 0u32;
                    for p in &c.netlist.cells[cell].pins {
                        if p.dir == PortDir::Output {
                            n += t[p.net as usize].wrapping_sub(prev[p.net as usize]);
                        }
                    }
                    let rate = n as f32 / dc as f32;
                    *a = *a * 0.6 + rate * 0.4;
                }
            } else if view.cycle < *cycle {
                self.activity.iter_mut().for_each(|a| *a = 0.0);
            }
        }
        self.prev_toggles = Some((view.cycle, Arc::clone(t)));
    }

    pub fn cell_color(&self, c: &Compiled, mods: &[ModuleInfo], stages: &StageMap, view: &View, cell: usize) -> [f32; 4] {
        let _ = mods;
        match self.color_by {
            ColorBy::Module => {
                let inst = c.netlist.cells[cell].instance as usize;
                let rgb3 = hsv(self.module_hue[inst], 0.55, 0.85);
                [rgb3[0], rgb3[1], rgb3[2], 1.0]
            }
            ColorBy::Stage => {
                let st = stages.stages[cell];
                if st == 0 {
                    [0.42, 0.44, 0.48, 1.0]
                } else {
                    let first = st.trailing_zeros() as usize;
                    rgb(STAGE_COLORS[first], 1.0)
                }
            }
            ColorBy::Activity => {
                let rate = match &self.window {
                    Some(w) if w.end > w.start => {
                        let n: u32 = c.netlist.cells[cell]
                            .pins
                            .iter()
                            .filter(|p| p.dir == PortDir::Output)
                            .map(|p| w.net_toggles[p.net as usize])
                            .sum();
                        n as f32 / (w.end - w.start) as f32
                    }
                    _ => self.activity[cell],
                };
                let a = (rate * 2.0).min(1.0);
                [0.15 + 0.85 * a, 0.12 + 0.6 * a * a, 0.18 * (1.0 - a), 1.0]
            }
            ColorBy::Value => {
                let out = c.netlist.cells[cell].pins.iter().find(|p| p.dir == PortDir::Output);
                let on = out.is_some_and(|p| view.nets.get(p.net as usize).copied().unwrap_or(0) != 0);
                if on { [0.95, 0.85, 0.3, 1.0] } else { [0.18, 0.2, 0.3, 1.0] }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        rs: Option<&egui_wgpu::RenderState>,
        c: &Compiled,
        mods: &[ModuleInfo],
        stages: &StageMap,
        view: &View,
        sel: &mut Selection,
        phys: Option<&Physical>,
    ) {
        self.update_activity(c, view);
        ui.horizontal(|ui| {
            ui.label("Color:");
            ui.selectable_value(&mut self.color_by, ColorBy::Stage, "pipeline stage");
            ui.selectable_value(&mut self.color_by, ColorBy::Module, "module");
            ui.selectable_value(&mut self.color_by, ColorBy::Activity, "activity");
            ui.selectable_value(&mut self.color_by, ColorBy::Value, "value");
            ui.separator();
            ui.checkbox(&mut self.show_pipeline, "pipeline");
            ui.checkbox(&mut self.show_source, "source");
            ui.checkbox(&mut self.show_nets, "nets of selection");
            if self.color_by == ColorBy::Activity {
                ui.separator();
                ui.add(egui::DragValue::new(&mut self.window_len).range(100..=5_000_000).speed(100.0).suffix(" cycles"));
                if ui.button("measure").on_hover_text("toggle counts of every net over this many cycles before the current one, by replay").clicked() {
                    self.request_window = Some((view.cycle.saturating_sub(self.window_len), view.cycle));
                }
                match &self.window {
                    Some(w) => {
                        ui.label(format!("window {}..{}", w.start, w.end));
                        if ui.small_button("live").clicked() {
                            self.window = None;
                        }
                    }
                    None => {
                        ui.label("live");
                    }
                }
            }
            ui.separator();
            let placed = self.physical.is_some();
            ui.add_enabled_ui(placed, |ui| {
                ui.checkbox(&mut self.use_physical, "physical placement")
                    .on_disabled_hover_text("no placed design loaded (run the ASIC flow)");
            });
        });

        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        // Camera control.
        let extent = self.layout().extent.w();
        if resp.dragged_by(egui::PointerButton::Primary) && !ui.input(|i| i.modifiers.shift) {
            let d = resp.drag_delta();
            self.camera.yaw += d.x * 0.008;
            self.camera.pitch = (self.camera.pitch + d.y * 0.008).clamp(0.08, 1.5);
        }
        if resp.dragged_by(egui::PointerButton::Secondary)
            || (resp.dragged_by(egui::PointerButton::Primary) && ui.input(|i| i.modifiers.shift))
        {
            let d = resp.drag_delta();
            let right = Vec3::new(-self.camera.yaw.sin(), 0.0, self.camera.yaw.cos());
            let fwd = Vec3::new(self.camera.yaw.cos(), 0.0, self.camera.yaw.sin());
            let k = self.camera.distance * 0.0015;
            self.camera.target += right * (-d.x * k) + fwd * (-d.y * k);
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.distance = (self.camera.distance * (1.0 - scroll * 0.0015)).clamp(extent * 0.02, extent * 4.0);
            }
        }
        let aspect = rect.width() / rect.height().max(1.0);
        let vp = self.camera.view_proj(aspect);

        // Cells whose outputs changed in the last cycle.
        let mut changed_cells: Vec<u32> = view
            .changed
            .iter()
            .filter_map(|&n| match c.driver[n as usize] {
                strata_gate::Driver::Node(k) => Some(c.node_cell[k as usize]),
                strata_gate::Driver::Flop(f, _) => Some(c.flop_cell[f as usize]),
                _ => None,
            })
            .collect();
        changed_cells.sort_unstable();
        changed_cells.dedup();

        // Build the scene.
        let mut renderer = self.renderer.take();
        let layout = self.layout();
        let n = c.netlist.cells.len();
        let mut boxes: Vec<BoxInstance> = Vec::with_capacity(n + 64);
        let mut now_active = vec![false; n];
        for &k in &changed_cells {
            now_active[k as usize] = true;
        }
        let physical_view = layout.placement == Placement::Physical;
        for (cell, &active) in now_active.iter().enumerate() {
            let p = layout.cell_pos[cell];
            let d = layout.cell_dims[cell];
            // Lower boxes in the physical view, so the metal layers above stay visible.
            let h = if physical_view {
                0.3 + (self.activity[cell] * 2.0).min(2.0) + if active { 1.0 } else { 0.0 }
            } else {
                0.6 + (self.activity[cell] * 10.0).min(14.0) + if active { 2.5 } else { 0.0 }
            };
            let mut color = self.cell_color(c, mods, stages, view, cell);
            if Some(cell as u32) == sel.cell {
                color = [1.0, 1.0, 1.0, 1.0];
            }
            boxes.push(BoxInstance { center: [p.x, 0.0, p.y], size: [d.x, h, d.y], color });
        }
        // In the physical placement, the cells the flow added (clock tree,
        // repair buffers, tie cells, antenna diodes) are shown dimmed; they are
        // not part of the simulated netlist. Fillers and well taps are omitted.
        let mut added = 0usize;
        if let (true, Some(ph)) = (physical_view, phys) {
            for (k, comp) in ph.layout.components.iter().enumerate() {
                if comp.physical_only || ph.comp_cell[k].is_some() {
                    continue;
                }
                let ctr = layout.to_scene(comp.x + comp.w / 2.0, comp.y + comp.h / 2.0);
                let s = layout.phys.map_or(1.0, |p| p.1);
                boxes.push(BoxInstance { center: [ctr.x, 0.0, ctr.y], size: [comp.w as f32 * s, 0.35, comp.h as f32 * s], color: [0.3, 0.3, 0.33, 1.0] });
                added += 1;
            }
        }
        let mut lines: Vec<LineVertex> = Vec::new();
        let rect_outline = |r: &Rect, y: f32, col: [f32; 4], lines: &mut Vec<LineVertex>| {
            let pts = [[r.x0, y, r.z0], [r.x1, y, r.z0], [r.x1, y, r.z1], [r.x0, y, r.z1]];
            for k in 0..4 {
                lines.push(LineVertex { pos: pts[k], color: col });
                lines.push(LineVertex { pos: pts[(k + 1) % 4], color: col });
            }
        };
        // Module outlines (depth 1 and the selected module).
        for (i, m) in mods.iter().enumerate() {
            let depth = m.path.matches('.').count() + usize::from(m.parent.is_some());
            let selected = sel.module == Some(i as u32);
            if depth <= 2 || selected {
                let col = if selected { [1.0, 1.0, 1.0, 1.0] } else { [0.55, 0.6, 0.7, 0.7] };
                rect_outline(&layout.module_rect[i], 0.05, col, &mut lines);
            }
        }
        // Pipeline plates, tied to the cells of their stage: to the region the
        // stage's logic occupies, and to the cells active in this cycle.
        if self.show_pipeline {
            for (st, &stage_color) in STAGE_COLORS.iter().enumerate() {
                let Some(region) = layout.stage_rect[st] else { continue };
                let valid = view.stages.get(st).is_some_and(|s| s.valid);
                let pr = plate_rect(extent, st);
                let y = stage_y(extent, st);
                let pc2 = pr.center();
                let col = rgb(stage_color, if valid { 0.55 } else { 0.18 });
                boxes.push(BoxInstance { center: [pc2.x, y, pc2.y], size: [pr.w(), extent * 0.006, pr.d()], color: col });
                let edge = rgb(stage_color, if valid { 0.95 } else { 0.35 });
                rect_outline(&pr, y + extent * 0.006, edge, &mut lines);
                rect_outline(&region, 0.1, rgb(stage_color, 0.5), &mut lines);
                let rc = region.center();
                lines.push(LineVertex { pos: [pc2.x, y, pc2.y], color: edge });
                lines.push(LineVertex { pos: [rc.x, 0.2, rc.y], color: [edge[0], edge[1], edge[2], 0.5] });
                if valid {
                    // Cells of this stage whose outputs changed in this cycle.
                    let active: Vec<u32> = changed_cells
                        .iter()
                        .copied()
                        .filter(|&c| stages.stages[c as usize] & (1 << st) != 0)
                        .collect();
                    let step = (active.len() / 48).max(1);
                    for &c in active.iter().step_by(step) {
                        let p = layout.cell_world(c);
                        lines.push(LineVertex { pos: [pc2.x, y, pc2.y], color: [edge[0], edge[1], edge[2], 0.35] });
                        lines.push(LineVertex { pos: [p.x, 1.0, p.z], color: [edge[0], edge[1], edge[2], 0.8] });
                    }
                }
            }
        }
        // Source layer: tied to each stage holding an instruction.
        let source_anchor = Vec3::new(0.0, SOURCE_HEIGHT * extent, 0.0);
        if self.show_source && self.show_pipeline {
            for (st, &stage_color) in STAGE_COLORS.iter().enumerate() {
                if view.stages.get(st).is_some_and(|s| s.valid) {
                    let c2 = plate_rect(extent, st).center();
                    let y = stage_y(extent, st) + extent * 0.006;
                    let col = rgb(stage_color, 0.8);
                    lines.push(LineVertex { pos: source_anchor.into(), color: col });
                    lines.push(LineVertex { pos: [c2.x, y, c2.y], color: col });
                }
            }
        }
        // Nets of the selected cell. Physical placement: the routed wires and
        // vias of each net from the layout, at the height of their metal layer.
        // Otherwise: arcs to the drivers of its inputs (blue) and the readers
        // of its outputs (orange).
        let mut routed = 0usize;
        if let (Some(cell), true, true, Some(ph)) = (sel.cell, self.show_nets, physical_view, phys) {
            for p in &c.netlist.cells[cell as usize].pins {
                let Some(net) = ph.net_of(cell, &p.name) else { continue };
                routed += 1;
                // Wires are drawn as bars (their real width, widened to stay
                // visible), vias as posts between the two layers they join.
                let s = layout.phys.map_or(1.0, |p| p.1);
                let thick = extent * 0.0015;
                for w in &net.wires {
                    let col = rgb(LAYER_COLORS[(w.layer as usize).min(5)], 1.0);
                    let y = layer_y(extent, w.layer);
                    let a = layout.to_scene(w.x0 as f64, w.y0 as f64);
                    let b = layout.to_scene(w.x1 as f64, w.y1 as f64);
                    let wd = (w.width * s).max(extent * 0.002);
                    let size = [(a.x - b.x).abs() + wd, thick, (a.y - b.y).abs() + wd];
                    boxes.push(BoxInstance { center: [(a.x + b.x) / 2.0, y - thick / 2.0, (a.y + b.y) / 2.0], size, color: col });
                }
                for v in &net.vias {
                    let a = layout.to_scene(v.x as f64, v.y as f64);
                    let (y0, y1) = (layer_y(extent, v.lower), layer_y(extent, v.lower + 1));
                    let wd = extent * 0.002;
                    boxes.push(BoxInstance { center: [a.x, y0, a.y], size: [wd, y1 - y0, wd], color: [0.92, 0.92, 0.92, 1.0] });
                }
            }
        } else if let (Some(cell), true) = (sel.cell, self.show_nets) {
            let here = layout.cell_world(cell) + Vec3::Y * 1.0;
            for p in &c.netlist.cells[cell as usize].pins {
                if p.dir == PortDir::Input {
                    if let Some(d) = inspect::driver_cell(c, p.net) {
                        let a = layout.cell_world(d) + Vec3::Y * 1.0;
                        arc(&mut lines, a, here, [0.4, 0.7, 1.0, 1.0]);
                    }
                } else {
                    for r in inspect::reader_cells(c, p.net).into_iter().take(64) {
                        let b = layout.cell_world(r) + Vec3::Y * 1.0;
                        arc(&mut lines, here, b, [1.0, 0.65, 0.2, 1.0]);
                    }
                }
            }
        }

        // Draw.
        let ppp = ui.ctx().pixels_per_point();
        let px = [(rect.width() * ppp) as u32, (rect.height() * ppp) as u32];
        if let Some(rs) = rs {
            let r = renderer.get_or_insert_with(|| Renderer3d::new(&rs.device));
            if let Some(tex) = r.render(rs, px, vp, &boxes, &lines, [0.055, 0.06, 0.075, 1.0]) {
                ui.painter().image(tex, rect, egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        } else {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "3D view needs the wgpu renderer", egui::FontId::proportional(14.0), Color32::GRAY);
        }

        // Labels.
        let project = |p: Vec3| -> Option<Pos2> {
            let clip = vp * Vec4::new(p.x, p.y, p.z, 1.0);
            if clip.w <= 0.0 {
                return None;
            }
            let ndc = clip.truncate() / clip.w;
            if ndc.x.abs() > 1.1 || ndc.y.abs() > 1.1 {
                return None;
            }
            Some(Pos2::new(rect.min.x + (ndc.x + 1.0) * 0.5 * rect.width(), rect.min.y + (1.0 - ndc.y) * 0.5 * rect.height()))
        };
        let painter = ui.painter_at(rect);
        let font = egui::FontId::monospace(11.0);
        let mut placed: Vec<egui::Rect> = Vec::new();
        for (i, m) in mods.iter().enumerate() {
            let r = &layout.module_rect[i];
            let (Some(a), Some(b)) = (project(Vec3::new(r.x0, 0.0, r.z0)), project(Vec3::new(r.x1, 0.0, r.z1))) else {
                continue;
            };
            if (a - b).length() > 90.0 && m.parent.is_some() {
                let c2 = r.center();
                if let Some(p) = project(Vec3::new(c2.x, 0.0, c2.y)) {
                    let name = m.path.rsplit('.').next().unwrap_or(&m.path);
                    let galley = painter.layout_no_wrap(name.to_string(), font.clone(), Color32::from_rgb(225, 230, 240));
                    let r = egui::Rect::from_center_size(p, galley.size() + egui::vec2(6.0, 2.0));
                    if placed.iter().all(|q| !q.intersects(r)) {
                        painter.rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(10, 12, 16, 170));
                        painter.galley(r.min + egui::vec2(3.0, 1.0), galley, Color32::WHITE);
                        placed.push(r);
                    }
                }
            }
        }
        if self.show_pipeline {
            for (st, &stage_color) in STAGE_COLORS.iter().enumerate() {
                let Some(sv) = view.stages.get(st) else { continue };
                let pr = plate_rect(extent, st);
                let z = if st % 2 == 0 { pr.z1 } else { pr.z0 };
                if let Some(p) = project(Vec3::new(pr.center().x, stage_y(extent, st), z)) {
                    let col = stage_color;
                    let text = if sv.valid { format!("{}  {:08x}\n{}", sv.name, sv.pc, sv.text) } else { format!("{}\nbubble", sv.name) };
                    let galley = painter.layout(text, egui::FontId::monospace(11.0), Color32::from_rgb(col[0], col[1], col[2]), 200.0);
                    let dy = if st % 2 == 0 { 6.0 } else { -galley.size().y - 12.0 };
                    let bg = egui::Rect::from_min_size(p + egui::vec2(-galley.size().x / 2.0 - 5.0, dy), galley.size() + egui::vec2(10.0, 6.0));
                    painter.rect_filled(bg, 3.0, Color32::from_rgba_unmultiplied(10, 12, 16, 210));
                    painter.galley(bg.min + egui::vec2(5.0, 3.0), galley, Color32::WHITE);
                }
            }
        }
        if self.show_source {
            if let Some(p) = project(source_anchor) {
                let w = view.stages.iter().rev().find(|s| s.valid);
                let text = match w.and_then(|s| s.source.as_ref().map(|l| (s, l))) {
                    Some((s, l)) => format!(
                        "{}  {}:{}",
                        s.symbol.clone().unwrap_or_default(),
                        l.file.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                        l.line
                    ),
                    None => "no source for the instructions in flight".into(),
                };
                let galley = painter.layout_no_wrap(text, egui::FontId::monospace(13.0), Color32::from_rgb(250, 250, 250));
                let bg = egui::Rect::from_center_size(p, galley.size() + egui::vec2(14.0, 8.0));
                painter.rect_filled(bg, 4.0, Color32::from_rgba_unmultiplied(40, 44, 60, 230));
                painter.rect_stroke(bg, 4.0, Stroke::new(1.0, Color32::from_gray(160)), egui::StrokeKind::Inside);
                painter.galley(bg.min + egui::vec2(7.0, 4.0), galley, Color32::WHITE);
            }
        }
        painter.text(
            rect.left_bottom() + egui::vec2(8.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "{} cells, {}. Drag: orbit. Shift/right-drag: pan. Scroll: zoom. Click: select.",
                n,
                if physical_view {
                    format!("placed as in the layout, plus {added} cells added by the flow (gray); {routed} routed nets of the selection shown")
                } else {
                    "placed by module (treemap)".to_string()
                }
            ),
            egui::FontId::proportional(11.0),
            Color32::from_gray(150),
        );

        // Picking: nearest cell box along the ray; else the module under the cursor.
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let ndc = Vec2::new(
                    (pos.x - rect.min.x) / rect.width() * 2.0 - 1.0,
                    1.0 - (pos.y - rect.min.y) / rect.height() * 2.0,
                );
                let inv = vp.inverse();
                let a = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
                let b = inv.project_point3(Vec3::new(ndc.x, ndc.y, 1.0));
                let dir = (b - a).normalize();
                let mut best: Option<(f32, u32)> = None;
                for (cell, bx) in boxes.iter().take(n).enumerate() {
                    let half = Vec3::new(bx.size[0] / 2.0, 0.0, bx.size[2] / 2.0);
                    let lo = Vec3::from(bx.center) - half;
                    let hi = Vec3::from(bx.center) + half + Vec3::Y * bx.size[1];
                    if let Some(t) = ray_box(a, dir, lo, hi) {
                        if best.is_none_or(|(bt, _)| t < bt) {
                            best = Some((t, cell as u32));
                        }
                    }
                }
                if let Some((_, cell)) = best {
                    sel.select_cell(c, cell);
                } else if dir.y.abs() > 1e-4 {
                    let t = -a.y / dir.y;
                    let hit = a + dir * t;
                    // Deepest module containing the point.
                    let mut found = None;
                    for (i, m) in mods.iter().enumerate() {
                        let r = &layout.module_rect[i];
                        if hit.x >= r.x0 && hit.x <= r.x1 && hit.z >= r.z0 && hit.z <= r.z1 {
                            let depth = m.path.matches('.').count();
                            if found.is_none_or(|(d, _)| depth >= d) {
                                found = Some((depth, i as u32));
                            }
                        }
                    }
                    sel.module = found.map(|(_, i)| i);
                    sel.cell = None;
                }
            }
        }
        let focus = resp.double_clicked().then_some(sel.cell).flatten().map(|cell| layout.cell_world(cell));
        self.renderer = renderer;
        if let Some(p) = focus {
            self.camera.target = p;
            self.camera.distance = extent * 0.12;
        }
    }

    /// Points the camera at a cell, close up.
    pub fn focus_cell(&mut self, cell: u32) {
        let p = self.layout().cell_world(cell);
        let extent = self.layout().extent.w();
        self.camera.target = p;
        self.camera.distance = extent * 0.18;
    }

    /// Points the camera at a module's region.
    pub fn focus_module(&mut self, i: u32) {
        let r = self.layout().module_rect[i as usize];
        let c2 = r.center();
        self.camera.target = Vec3::new(c2.x, 0.0, c2.y);
        self.camera.distance = (r.w().max(r.d()) * 1.8).max(20.0);
    }
}

/// Height of the pipeline layer.
fn stage_y(extent: f32, _st: usize) -> f32 {
    extent * STAGE_HEIGHT
}

/// The plate of stage `st`: the five plates form a row, in pipeline order,
/// over the middle of the floor.
fn plate_rect(extent: f32, st: usize) -> Rect {
    let w = extent * 0.15;
    let gap = extent * 0.035;
    let x0 = -2.5 * w - 2.0 * gap + st as f32 * (w + gap);
    Rect { x0, z0: -extent * 0.06, x1: x0 + w, z1: extent * 0.06 }
}

fn arc(lines: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [f32; 4]) {
    let lift = (a - b).length() * 0.25 + 4.0;
    let mut prev = a;
    for k in 1..=8 {
        let t = k as f32 / 8.0;
        let p = a.lerp(b, t) + Vec3::Y * (lift * 4.0 * t * (1.0 - t));
        lines.push(LineVertex { pos: prev.into(), color });
        lines.push(LineVertex { pos: p.into(), color });
        prev = p;
    }
}

fn ray_box(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3) -> Option<f32> {
    let inv = d.recip();
    let t1 = (lo - o) * inv;
    let t2 = (hi - o) * inv;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    (tmax >= tmin.max(0.0)).then_some(tmin.max(0.0))
}
