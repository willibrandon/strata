//! The physical layout from the place-and-route flow, seen from above: every
//! placed cell with the same coloring as the 3D view, the routed wires of the
//! selected cell's nets, and the I/O pins.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke};
use glam::Vec2;

use crate::app::Statics;
use crate::physical::Physical;
use crate::scene_view::{ColorBy, SceneView, LAYER_COLORS};
use crate::view::View;
use crate::Selection;

pub struct LayoutView {
    /// Pixels per µm.
    zoom: f32,
    /// Layout point (µm) at the center of the view.
    center: Vec2,
    fitted: bool,
    show_filler: bool,
    show_all_nets: bool,
    /// Components of each placement row, sorted by x, for picking.
    rows: Vec<(f64, Vec<u32>)>,
}

impl Default for LayoutView {
    fn default() -> Self {
        LayoutView { zoom: 1.0, center: Vec2::ZERO, fitted: false, show_filler: false, show_all_nets: false, rows: Vec::new() }
    }
}

fn c32(c: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 255.0) as u8)
}

impl LayoutView {
    /// Centers the view on a layout component at a close zoom.
    pub fn focus(&mut self, ph: &Physical, comp: u32) {
        let c = &ph.layout.components[comp as usize];
        self.center = Vec2::new((c.x + c.w / 2.0) as f32, (c.y + c.h / 2.0) as f32);
        self.zoom = 12.0;
        self.fitted = true;
    }

    fn index(&mut self, ph: &Physical) {
        let mut rows: Vec<(f64, Vec<u32>)> = ph.layout.rows.iter().map(|r| (r[1], Vec::new())).collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (k, c) in ph.layout.components.iter().enumerate() {
            let i = rows.partition_point(|r| r.0 <= c.y + 1e-6).saturating_sub(1);
            if let Some(r) = rows.get_mut(i) {
                r.1.push(k as u32);
            }
        }
        for r in &mut rows {
            r.1.sort_by(|&a, &b| ph.layout.components[a as usize].x.total_cmp(&ph.layout.components[b as usize].x));
        }
        self.rows = rows;
    }

    /// The component under a layout point.
    fn pick(&self, ph: &Physical, p: Vec2) -> Option<u32> {
        let i = self.rows.partition_point(|r| r.0 <= p.y as f64).checked_sub(1)?;
        let row = &self.rows[i].1;
        let comps = &ph.layout.components;
        let j = row.partition_point(|&k| comps[k as usize].x <= p.x as f64).checked_sub(1)?;
        let k = row[j];
        let c = &comps[k as usize];
        (p.x as f64 <= c.x + c.w && p.y as f64 <= c.y + c.h).then_some(k)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, st: &Statics, scene: &mut SceneView, view: &View, sel: &mut Selection) {
        let Some(ph) = st.physical.as_deref() else {
            ui.label("No layout loaded. Run scripts/physical.py (OpenROAD place and route of the gate netlist), then restart the debugger, or pass --layout <dir>.");
            return;
        };
        if self.rows.is_empty() {
            self.index(ph);
        }
        let l = &ph.layout;
        // Wrapped, so a narrow panel does not widen the drawing area.
        ui.horizontal_wrapped(|ui| {
            ui.label("Color:");
            ui.selectable_value(&mut scene.color_by, ColorBy::Stage, "pipeline stage");
            ui.selectable_value(&mut scene.color_by, ColorBy::Module, "module");
            ui.selectable_value(&mut scene.color_by, ColorBy::Activity, "activity");
            ui.selectable_value(&mut scene.color_by, ColorBy::Value, "value");
            ui.separator();
            if ui.button("fit").clicked() {
                self.fitted = false;
            }
            ui.checkbox(&mut self.show_filler, "fillers and taps");
            ui.checkbox(&mut self.show_all_nets, "all routing (slow)");
            ui.separator();
            for (i, layer) in l.layers.iter().enumerate() {
                let c = LAYER_COLORS[i.min(5)];
                ui.label(egui::RichText::new(&layer.name).color(Color32::from_rgb(c[0], c[1], c[2])).monospace());
            }
            ui.separator();
            ui.label(format!(
                "die {:.0} x {:.0} µm, {} components ({} simulated cells)",
                l.die[2] - l.die[0],
                l.die[3] - l.die[1],
                l.components.len(),
                ph.matched()
            ));
        });
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if !self.fitted && rect.width() > 10.0 {
            let (w, h) = ((l.die[2] - l.die[0]) as f32, (l.die[3] - l.die[1]) as f32);
            self.zoom = (rect.width() / w).min(rect.height() / h) * 0.95;
            self.center = Vec2::new((l.die[0] + l.die[2]) as f32 / 2.0, (l.die[1] + l.die[3]) as f32 / 2.0);
            self.fitted = true;
        }
        // Navigation: drag pans, scroll zooms about the cursor.
        if resp.dragged() {
            let d = resp.drag_delta();
            self.center -= Vec2::new(d.x, -d.y) / self.zoom;
        }
        let to_screen = |zoom: f32, center: Vec2, x: f64, y: f64| -> Pos2 {
            Pos2::new(rect.center().x + (x as f32 - center.x) * zoom, rect.center().y - (y as f32 - center.y) * zoom)
        };
        let to_layout = |zoom: f32, center: Vec2, p: Pos2| -> Vec2 {
            Vec2::new(center.x + (p.x - rect.center().x) / zoom, center.y - (p.y - rect.center().y) / zoom)
        };
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                if let Some(m) = ui.input(|i| i.pointer.hover_pos()) {
                    let before = to_layout(self.zoom, self.center, m);
                    self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.2, 400.0);
                    let after = to_layout(self.zoom, self.center, m);
                    self.center += before - after;
                }
            }
        }
        let (zoom, center) = (self.zoom, self.center);
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, Color32::from_rgb(12, 13, 17));
        let die = Rect::from_two_pos(to_screen(zoom, center, l.die[0], l.die[1]), to_screen(zoom, center, l.die[2], l.die[3]));
        p.rect_stroke(die, 0.0, Stroke::new(1.0, Color32::from_gray(120)), egui::StrokeKind::Outside);

        // Cells.
        let c = &st.compiled;
        for (k, comp) in l.components.iter().enumerate() {
            if comp.physical_only && !self.show_filler {
                continue;
            }
            let r = Rect::from_two_pos(to_screen(zoom, center, comp.x, comp.y), to_screen(zoom, center, comp.x + comp.w, comp.y + comp.h));
            if !r.intersects(rect) {
                continue;
            }
            let fill = match ph.comp_cell[k] {
                Some(cell) if Some(cell) == sel.cell => Color32::WHITE,
                Some(cell) => c32(scene.cell_color(c, &st.mods, &st.stages, view, cell as usize)),
                None if comp.physical_only => Color32::from_gray(30),
                None => Color32::from_gray(80),
            };
            let r = if zoom > 3.0 { r.shrink(0.5) } else { r };
            p.rect_filled(r, 0.0, fill);
        }

        // Routing: the selected cell's nets (or everything, on request).
        let mut nets: Vec<&strata_layout::Net> = Vec::new();
        if self.show_all_nets {
            nets.extend(l.nets.iter());
        } else if let Some(cell) = sel.cell {
            for pin in &c.netlist.cells[cell as usize].pins {
                if let Some(n) = ph.net_of(cell, &pin.name) {
                    nets.push(n);
                }
            }
        }
        for net in &nets {
            for w in &net.wires {
                let a = to_screen(zoom, center, w.x0 as f64, w.y0 as f64);
                let b = to_screen(zoom, center, w.x1 as f64, w.y1 as f64);
                let col = LAYER_COLORS[(w.layer as usize).min(5)];
                let width = (w.width * zoom).max(1.0);
                p.line_segment([a, b], Stroke::new(width, Color32::from_rgba_unmultiplied(col[0], col[1], col[2], 200)));
            }
            for v in &net.vias {
                let a = to_screen(zoom, center, v.x as f64, v.y as f64);
                let s = (0.2 * zoom).max(2.0);
                p.rect_stroke(Rect::from_center_size(a, egui::vec2(s, s)), 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Middle);
            }
        }
        // I/O pins.
        for pin in &l.pins {
            let a = to_screen(zoom, center, pin.x, pin.y);
            if rect.contains(a) {
                p.circle_filled(a, 2.5, Color32::from_rgb(120, 230, 120));
                if zoom > 6.0 {
                    p.text(a + egui::vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, &pin.name, egui::FontId::monospace(10.0), Color32::from_rgb(160, 240, 160));
                }
            }
        }
        // Labels of the selected cell's nets.
        if let Some(cell) = sel.cell {
            if let Some(k) = ph.cell_comp[cell as usize] {
                let comp = &l.components[k as usize];
                let a = to_screen(zoom, center, comp.x + comp.w, comp.y + comp.h);
                let mut text = format!("{}  {}", comp.name, comp.cell.trim_start_matches("sky130_fd_sc_hd__"));
                for pin in &c.netlist.cells[cell as usize].pins {
                    if let Some(n) = ph.net_of(cell, &pin.name) {
                        text += &format!(
                            "\n{:>4} {}  {:.0} µm routed{}",
                            pin.name,
                            n.name,
                            n.length(),
                            n.cap.map(|c| format!(", {:.1} fF wire", c * 1000.0)).unwrap_or_default()
                        );
                    }
                }
                let g = p.layout_no_wrap(text, egui::FontId::monospace(11.0), Color32::WHITE);
                let size = g.size() + egui::vec2(8.0, 6.0);
                // Keep the label inside the view.
                let min = Pos2::new(
                    (a.x + 6.0).min(rect.right() - size.x - 4.0).max(rect.left() + 4.0),
                    (a.y - 6.0).min(rect.bottom() - size.y - 4.0).max(rect.top() + 4.0),
                );
                let bg = Rect::from_min_size(min, size);
                p.rect_filled(bg, 3.0, Color32::from_rgba_unmultiplied(10, 12, 16, 220));
                p.galley(bg.min + egui::vec2(4.0, 3.0), g, Color32::WHITE);
            }
        }

        // Hover and selection.
        if let Some(m) = resp.hover_pos() {
            if let Some(k) = self.pick(ph, to_layout(zoom, center, m)) {
                let comp = &l.components[k as usize];
                let what = if ph.comp_cell[k as usize].is_some() { "simulated cell" } else if comp.physical_only { "filler or tap" } else { "added by the flow" };
                resp.clone().on_hover_text(format!("{}\n{}\n{}", comp.name, comp.cell, what));
            }
        }
        if resp.clicked() {
            if let Some(m) = resp.interact_pointer_pos() {
                match self.pick(ph, to_layout(zoom, center, m)).and_then(|k| ph.comp_cell[k as usize]) {
                    Some(cell) => sel.select_cell(c, cell),
                    None => sel.cell = None,
                }
            }
        }
        if resp.double_clicked() {
            if let Some(k) = sel.cell.and_then(|cell| ph.cell_comp[cell as usize]) {
                let comp = &l.components[k as usize];
                self.center = Vec2::new((comp.x + comp.w / 2.0) as f32, (comp.y + comp.h / 2.0) as f32);
                self.zoom = self.zoom.max(40.0);
            }
        }
        p.text(
            rect.left_bottom() + egui::vec2(8.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{} (OpenROAD, sky130). Drag: pan. Scroll: zoom. Click: select. Double-click: zoom to selection.", ph.dir.join("6_final.def").display()),
            egui::FontId::proportional(11.0),
            Color32::from_gray(150),
        );
    }
}
