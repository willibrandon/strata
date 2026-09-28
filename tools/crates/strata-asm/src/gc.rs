//! Garbage collection of unreferenced input sections (`--gc-sections`).
//!
//! An input section is live if it is a root or if a live section refers to
//! it. Roots are the section defining the entry symbol, every section
//! matched by a `KEEP(...)` layout pattern, and the sections defining the
//! `--keep-symbol` symbols. A section refers to another through any of its
//! fixups: every symbol or location an expression mentions counts,
//! including both ends of a label difference and the targets of
//! `%pcrel_hi`/`%pcrel_lo`. Symbols are followed to the section that
//! defines them: through the global symbol table, `.set` definitions,
//! common symbols, and layout `define`s that name program symbols.

use crate::diag::Diags;
use crate::expr::{Expr, SecRef, SymId, MAX_DEPTH};
use crate::layout::{LExpr, Layout};
use crate::link::GDef;
use crate::object::{Bind, Object, SymDef, Symbol};
use std::collections::HashMap;

/// Resolves symbols to the input sections they depend on.
struct Graph<'a> {
    objects: &'a [Object],
    globals: &'a HashMap<String, GDef>,
    /// Program symbols mentioned by each layout `define`.
    defines: HashMap<&'a str, Vec<&'a str>>,
}

fn define_symbols<'a>(e: &'a LExpr, out: &mut Vec<&'a str>) {
    match e {
        LExpr::Sym(name) => out.push(name),
        LExpr::Align(a, b) | LExpr::Bin(_, a, b) => {
            define_symbols(a, out);
            define_symbols(b, out);
        }
        LExpr::Neg(a) => define_symbols(a, out),
        LExpr::Num(_) | LExpr::Query(..) => {}
    }
}

impl Graph<'_> {
    /// Sections that symbol `id` of object `obj` depends on, following the
    /// same resolution rules as the linker.
    fn symbol(&self, obj: u32, id: SymId, depth: u32, out: &mut Vec<SecRef>) {
        if depth > MAX_DEPTH {
            return;
        }
        let s = &self.objects[obj as usize].symbols[id as usize];
        if s.bind != Bind::Local || !s.is_defined() {
            if self.global(&s.name, depth, out) {
                return;
            }
            if !s.is_defined() {
                // A section name stands for the start of that section.
                if let Some(sec) = self.objects[obj as usize].find_section(&s.name) {
                    out.push(SecRef { obj, sec });
                }
                return;
            }
        }
        self.definition(obj, s, depth, out);
    }

    /// Sections that global `name` depends on; false if there is no global
    /// definition (program or layout).
    fn global(&self, name: &str, depth: u32, out: &mut Vec<SecRef>) -> bool {
        match self.globals.get(name) {
            Some(GDef::Sym { obj, sym, .. } | GDef::Common { obj, sym, .. }) => {
                let s = &self.objects[*obj as usize].symbols[*sym as usize];
                self.definition(*obj, s, depth, out);
                true
            }
            Some(GDef::Layout(_)) => true,
            None => match self.defines.get(name) {
                Some(names) => {
                    for n in names {
                        self.global(n, depth + 1, out);
                    }
                    true
                }
                None => false,
            },
        }
    }

    fn definition(&self, obj: u32, s: &Symbol, depth: u32, out: &mut Vec<SecRef>) {
        match &s.def {
            SymDef::Label { sec, .. } => out.push(SecRef { obj, sec: *sec }),
            SymDef::Equ(e) => self.expr(obj, e, depth + 1, out),
            SymDef::Common { .. } | SymDef::Undef => {}
        }
    }

    /// Sections an expression of object `obj` refers to.
    fn expr(&self, obj: u32, e: &Expr, depth: u32, out: &mut Vec<SecRef>) {
        match e {
            Expr::Num(_) => {}
            Expr::Sym(id) => self.symbol(obj, *id, depth, out),
            Expr::Loc(sec, _) => out.push(*sec),
            Expr::Unary(_, a) => self.expr(obj, a, depth, out),
            Expr::Binary(_, a, b) => {
                self.expr(obj, a, depth, out);
                self.expr(obj, b, depth, out);
            }
        }
    }

    /// Sections defining `name`: its global definition, or else every local
    /// symbol of that name. `None` if no symbol of that name is defined.
    fn named(&self, name: &str) -> Option<Vec<SecRef>> {
        let mut out = Vec::new();
        if self.global(name, 0, &mut out) {
            return Some(out);
        }
        let mut found = false;
        for (oi, obj) in self.objects.iter().enumerate() {
            if let Some(id) = obj.find_symbol(name) {
                let s = &obj.symbols[id as usize];
                if s.is_defined() && !s.cmdline {
                    found = true;
                    self.definition(oi as u32, s, 0, &mut out);
                }
            }
        }
        found.then_some(out)
    }
}

/// What `--gc-sections` needs besides the objects and the symbol table.
pub(crate) struct Roots<'a> {
    pub layout: &'a Layout,
    /// `(rule, pattern)` matched by every input section (`None`: no match or
    /// discarded).
    pub matches: &'a [Vec<Option<(usize, usize)>>],
    pub keep_symbols: &'a [String],
}

/// Marks the live input sections. Errors (unknown `--keep-symbol`, no
/// roots at all) are reported to `diags`.
pub(crate) fn live_sections(
    objects: &[Object],
    globals: &HashMap<String, GDef>,
    roots: &Roots,
    diags: &mut Diags,
) -> Vec<Vec<bool>> {
    let mut defines = HashMap::new();
    for d in &roots.layout.defines {
        let mut names = Vec::new();
        define_symbols(&d.expr, &mut names);
        defines.insert(d.name.as_str(), names);
    }
    let graph = Graph {
        objects,
        globals,
        defines,
    };

    let mut work: Vec<SecRef> = Vec::new();
    // The entry symbol, as the linker chooses it.
    let entry = roots
        .layout
        .entry
        .as_ref()
        .map_or("_start", |(name, _)| name.as_str());
    graph.global(entry, 0, &mut work);
    for (oi, per_obj) in roots.matches.iter().enumerate() {
        for (si, m) in per_obj.iter().enumerate() {
            if let Some((ri, pi)) = *m {
                if roots.layout.outputs[ri].patterns[pi].keep {
                    work.push(SecRef {
                        obj: oi as u32,
                        sec: si as u32,
                    });
                }
            }
        }
    }
    for name in roots.keep_symbols {
        match graph.named(name) {
            Some(secs) => work.extend(secs),
            None => diags.error_global(format!("--keep-symbol: symbol '{name}' is not defined")),
        }
    }
    if work.is_empty() {
        diags.error_global(
            "--gc-sections found no roots: define the entry symbol, use KEEP() in the layout, or pass --keep-symbol",
        );
    }

    let mut live: Vec<Vec<bool>> = objects
        .iter()
        .map(|o| vec![false; o.sections.len()])
        .collect();
    let mut refs = Vec::new();
    while let Some(s) = work.pop() {
        let sec = &objects[s.obj as usize].sections[s.sec as usize];
        if sec.discarded || live[s.obj as usize][s.sec as usize] {
            continue;
        }
        live[s.obj as usize][s.sec as usize] = true;
        for f in &sec.fixups {
            refs.clear();
            graph.expr(s.obj, &f.expr, 0, &mut refs);
            work.extend(
                refs.iter()
                    .filter(|r| !live[r.obj as usize][r.sec as usize]),
            );
        }
    }
    live
}
