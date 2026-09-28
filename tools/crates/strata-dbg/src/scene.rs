//! The 3D inspection scene: where each object of the inspection model sits.
//!
//! Layers, from the floor up:
//!   * cells: every netlist cell is a box on the floor, placed either by the
//!     physical layout (when a placed design is loaded) or by a treemap of
//!     the RTL module hierarchy; height and brightness show activity;
//!   * modules: each RTL instance is a region outlined on the floor;
//!   * pipeline: five stage plates (F D X M W) hover over the cells that
//!     implement each stage and are tied to them, each labeled with the
//!     instruction in that stage;
//!   * software: the current source line hovers above the pipeline.
//!
//! Nothing in the scene is decorative: every object is an entity of the
//! inspection model and selecting it selects that entity.

use glam::{Vec2, Vec3};
use strata_debug::inspect::{ModuleInfo, StageMap};
use strata_gate::Compiled;

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x0: f32,
    pub z0: f32,
    pub x1: f32,
    pub z1: f32,
}

impl Rect {
    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn d(&self) -> f32 {
        self.z1 - self.z0
    }
    pub fn center(&self) -> Vec2 {
        Vec2::new((self.x0 + self.x1) / 2.0, (self.z0 + self.z1) / 2.0)
    }
    fn union(a: Option<Rect>, p: Vec2, r: f32) -> Rect {
        match a {
            None => Rect { x0: p.x - r, z0: p.y - r, x1: p.x + r, z1: p.y + r },
            Some(a) => Rect { x0: a.x0.min(p.x - r), z0: a.z0.min(p.y - r), x1: a.x1.max(p.x + r), z1: a.z1.max(p.y + r) },
        }
    }
}

/// Where the cell positions came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Treemap,
    Physical,
}

pub struct SceneLayout {
    /// Floor position (x, z) of each cell's center.
    pub cell_pos: Vec<Vec2>,
    /// Footprint of each cell box (edge length).
    pub cell_size: Vec<f32>,
    pub module_rect: Vec<Rect>,
    pub stage_rect: [Option<Rect>; 5],
    pub extent: Rect,
    pub placement: Placement,
}

/// Heights of the pipeline and source layers, as fractions of the floor size.
pub const STAGE_HEIGHT: f32 = 0.42;
pub const SOURCE_HEIGHT: f32 = 0.78;

impl SceneLayout {
    /// Treemap of the module hierarchy on a square floor.
    pub fn treemap(c: &Compiled, mods: &[ModuleInfo], stages: &StageMap) -> SceneLayout {
        let n = c.netlist.cells.len();
        let side = (n as f32).sqrt() * 1.35;
        let mut cell_pos = vec![Vec2::ZERO; n];
        let mut module_rect = vec![Rect { x0: 0.0, z0: 0.0, x1: 0.0, z1: 0.0 }; mods.len()];
        let root = Rect { x0: -side / 2.0, z0: -side / 2.0, x1: side / 2.0, z1: side / 2.0 };
        place(c, mods, stages, 0, root, true, &mut cell_pos, &mut module_rect);
        let mut layout = SceneLayout {
            cell_pos,
            cell_size: vec![0.8; n],
            module_rect,
            stage_rect: [None; 5],
            extent: root,
            placement: Placement::Treemap,
        };
        layout.compute_stage_rects(stages);
        layout
    }

    /// Positions from a placed design (cell name -> x, z, size), scaled to
    /// the same floor size as the treemap. Cells missing from the placement
    /// keep their treemap position.
    pub fn with_physical(&self, placed: &[(u32, f32, f32, f32)], stages: &StageMap, mods: &[ModuleInfo]) -> SceneLayout {
        let mut out = SceneLayout {
            cell_pos: self.cell_pos.clone(),
            cell_size: self.cell_size.clone(),
            module_rect: self.module_rect.clone(),
            stage_rect: [None; 5],
            extent: self.extent,
            placement: Placement::Physical,
        };
        if placed.is_empty() {
            return out;
        }
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for &(_, x, z, _) in placed {
            lo = lo.min(Vec2::new(x, z));
            hi = hi.max(Vec2::new(x, z));
        }
        let span = (hi - lo).max_element().max(1e-3);
        let target = self.extent.w();
        let scale = target / span;
        let offset = -(lo + hi) / 2.0;
        for &(cell, x, z, size) in placed {
            out.cell_pos[cell as usize] = (Vec2::new(x, z) + offset) * scale;
            out.cell_size[cell as usize] = (size * scale).clamp(0.3, 4.0);
        }
        // Module regions become the bounding boxes of their cells.
        for (i, m) in mods.iter().enumerate() {
            let mut r: Option<Rect> = None;
            for &c in &m.cells {
                r = Some(Rect::union(r, out.cell_pos[c as usize], 0.5));
            }
            if let Some(r) = r {
                out.module_rect[i] = r;
            }
        }
        for i in (0..mods.len()).rev() {
            for &ch in &mods[i].children {
                let a = out.module_rect[i];
                let b = out.module_rect[ch as usize];
                if b.w() > 0.0 {
                    out.module_rect[i] = if a.w() > 0.0 {
                        Rect { x0: a.x0.min(b.x0), z0: a.z0.min(b.z0), x1: a.x1.max(b.x1), z1: a.z1.max(b.z1) }
                    } else {
                        b
                    };
                }
            }
        }
        out.compute_stage_rects(stages);
        out
    }

    fn compute_stage_rects(&mut self, stages: &StageMap) {
        for st in 0..5 {
            // Robust bounds: the central 90% of the stage's cells.
            let mut xs: Vec<f32> = Vec::new();
            let mut zs: Vec<f32> = Vec::new();
            for c in stages.cells_of(st) {
                xs.push(self.cell_pos[c as usize].x);
                zs.push(self.cell_pos[c as usize].y);
            }
            if xs.is_empty() {
                continue;
            }
            xs.sort_by(f32::total_cmp);
            zs.sort_by(f32::total_cmp);
            let q = |v: &[f32], f: f32| v[((v.len() - 1) as f32 * f) as usize];
            self.stage_rect[st] = Some(Rect { x0: q(&xs, 0.05), z0: q(&zs, 0.05), x1: q(&xs, 0.95), z1: q(&zs, 0.95) });
        }
    }

    pub fn cell_world(&self, cell: u32) -> Vec3 {
        let p = self.cell_pos[cell as usize];
        Vec3::new(p.x, 0.0, p.y)
    }
}

/// Places instance `i` (its own cells and its children) inside `r`,
/// splitting along the longer side in proportion to cell counts.
#[allow(clippy::too_many_arguments)]
fn place(
    c: &Compiled,
    mods: &[ModuleInfo],
    stages: &StageMap,
    i: usize,
    r: Rect,
    _horizontal: bool,
    pos: &mut [Vec2],
    rects: &mut [Rect],
) {
    const GAP: f32 = 1.5;
    rects[i] = r;
    let m = &mods[i];
    let inner = Rect { x0: r.x0 + GAP, z0: r.z0 + GAP, x1: r.x1 - GAP, z1: r.z1 - GAP };
    // Items: own cells first, then children, largest first.
    let mut items: Vec<(Option<usize>, usize)> = Vec::new();
    if !m.cells.is_empty() {
        items.push((None, m.cells.len()));
    }
    let mut kids: Vec<usize> = m.children.iter().map(|&k| k as usize).collect();
    kids.sort_by_key(|&k| std::cmp::Reverse(mods[k].total_cells));
    items.extend(kids.iter().map(|&k| (Some(k), mods[k].total_cells.max(1))));
    let total: usize = items.iter().map(|x| x.1).sum::<usize>().max(1);
    let split_x = inner.w() >= inner.d();
    let mut at = if split_x { inner.x0 } else { inner.z0 };
    let len = if split_x { inner.w() } else { inner.d() };
    for (item, weight) in items {
        let span = len * weight as f32 / total as f32;
        let sub = if split_x {
            Rect { x0: at, z0: inner.z0, x1: at + span, z1: inner.z1 }
        } else {
            Rect { x0: inner.x0, z0: at, x1: inner.x1, z1: at + span }
        };
        at += span;
        match item {
            Some(k) => place(c, mods, stages, k, sub, !split_x, pos, rects),
            None => pack(c, &m.cells, stages, sub, pos),
        }
    }
}

/// Packs cells into a grid filling `r`, grouped by pipeline stage and type.
fn pack(c: &Compiled, cells: &[u32], stages: &StageMap, r: Rect, pos: &mut [Vec2]) {
    let mut order: Vec<u32> = cells.to_vec();
    order.sort_by_key(|&k| (stages.stages[k as usize], c.cell_def[k as usize]));
    let n = order.len().max(1) as f32;
    let aspect = (r.w() / r.d().max(1e-3)).max(1e-3);
    let cols = (n * aspect).sqrt().ceil().max(1.0);
    let rows = (n / cols).ceil().max(1.0);
    let (dx, dz) = (r.w() / cols, r.d() / rows);
    for (k, &cell) in order.iter().enumerate() {
        let col = (k as f32 % cols).floor();
        let row = (k as f32 / cols).floor();
        pos[cell as usize] = Vec2::new(r.x0 + (col + 0.5) * dx, r.z0 + (row + 0.5) * dz);
    }
}
