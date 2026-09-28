//! The linker: resolves symbols across objects, places input sections
//! according to a [`Layout`], evaluates layout definitions, and applies all
//! fixups, producing a [`Program`].

use crate::diag::{Diags, FileId, Loc};
use crate::expr::{self, Env, SecRef, SymId, Value};
use crate::layout::{align_up_i64, bin, glob_match, LExpr, Layout, Pattern};
use crate::object::{Bind, FixKind, Fixup, Object, SecFlags, Section, SymDef, SymKind, Symbol};
use std::collections::HashMap;
use strata_isa::encode::{
    fits_signed, hi20, lo12, with_b_imm, with_i_imm, with_j_imm, with_s_imm, with_u_imm,
};

/// An input section placed in an output section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Input file index (the synthetic `COMMON` object comes last).
    pub obj: u32,
    /// Section index within that object.
    pub sec: u32,
    /// Input section name.
    pub name: String,
    /// Final address.
    pub addr: u32,
    /// Size in bytes.
    pub size: u32,
    /// Alignment in bytes.
    pub align: u64,
}

/// A placed output section.
#[derive(Clone, Debug)]
pub struct OutSection {
    /// Name from the layout's `output` rule.
    pub name: String,
    /// Memory region it lives in.
    pub region: String,
    /// Start address.
    pub addr: u32,
    /// Size in bytes.
    pub size: u32,
    /// Alignment in bytes.
    pub align: u64,
    /// Union of the inputs' `a w x` flags; `M`/`S` only if all inputs have them.
    pub flags: SecFlags,
    /// Holds no file bytes.
    pub nobits: bool,
    /// Contents (empty for nobits sections).
    pub data: Vec<u8>,
    /// Input sections in placement order.
    pub inputs: Vec<Placed>,
}

/// A symbol of the linked program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutSymbol {
    /// Symbol name.
    pub name: String,
    /// Value (an address, or a number for absolute symbols).
    pub addr: u32,
    /// Size from `.size` (0 if none).
    pub size: u32,
    /// Type from `.type`.
    pub kind: SymKind,
    /// Binding.
    pub bind: Bind,
    /// Index into [`Program::sections`]; `None` for absolute symbols.
    pub section: Option<usize>,
    /// Index of the input file that defined it; `None` for layout symbols.
    pub file: Option<usize>,
}

/// A `.loc` row at a final address (`line == 0` marks the end of a sequence).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEntry {
    /// Address of the first instruction the row applies to.
    pub addr: u32,
    /// Source file.
    pub file: FileId,
    /// Source line (0: no source line).
    pub line: u32,
    /// Source column (0: unknown).
    pub col: u32,
    /// DWARF `is_stmt` flag.
    pub is_stmt: bool,
}

/// The assembler-input location of one emitted instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AsmLine {
    /// Address of the instruction.
    pub addr: u32,
    /// Assembler input file (after line markers).
    pub file: FileId,
    /// Line in that file (the invocation line for macro expansions).
    pub line: u32,
}

/// One listing line with final address and bytes.
#[derive(Clone, Debug)]
pub struct ListingLine {
    /// Source location of the statement.
    pub loc: Loc,
    /// Statement text (comments removed; expanded for macros).
    pub text: String,
    /// Address, if the statement is in a placed section.
    pub addr: Option<u32>,
    /// Bytes the statement produced (after relocation).
    pub bytes: Vec<u8>,
    /// True if the bytes are instructions.
    pub code: bool,
}

/// Memory region usage.
#[derive(Clone, Debug)]
pub struct RegionUse {
    /// Region name.
    pub name: String,
    /// First address.
    pub origin: u64,
    /// Size in bytes.
    pub length: u64,
    /// Bytes occupied by output sections.
    pub used: u64,
}

/// An input file of the program.
#[derive(Clone, Debug)]
pub struct InputFile {
    /// Path as given on the command line.
    pub path: String,
    /// Its id in the run's [`FileTable`](crate::diag::FileTable).
    pub file: FileId,
}

/// A fully linked program.
#[derive(Clone, Debug)]
pub struct Program {
    /// Output sections in layout order (empty ones included).
    pub sections: Vec<OutSection>,
    /// Symbols sorted by address, then name.
    pub symbols: Vec<OutSymbol>,
    /// Entry point address.
    pub entry: u32,
    /// `.loc` rows sorted by address.
    pub lines: Vec<LineEntry>,
    /// One row per emitted instruction, sorted by address.
    pub asm_lines: Vec<AsmLine>,
    /// Input files in command-line order.
    pub inputs: Vec<InputFile>,
    /// Listing lines per input file.
    pub listing: Vec<Vec<ListingLine>>,
    /// Usage of every memory region.
    pub regions: Vec<RegionUse>,
    /// Non-empty input sections removed by `--gc-sections`.
    pub gc_removed: Vec<RemovedSection>,
}

/// An input section removed by `--gc-sections`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovedSection {
    /// Input file path (`COMMON` for common symbols).
    pub path: String,
    /// Section name (the symbol name for common symbols).
    pub name: String,
    /// Size in bytes.
    pub size: u64,
}

/// Linker settings.
#[derive(Clone, Debug, Default)]
pub struct LinkOptions {
    /// `-D` symbols (visible to layout expressions).
    pub defines: Vec<(String, i64)>,
    /// Remove input sections not reachable from the roots (`--gc-sections`).
    pub gc_sections: bool,
    /// Extra `--gc-sections` roots: the sections defining these symbols.
    pub keep_symbols: Vec<String>,
}

impl Program {
    /// Output section containing `addr`, if any (non-empty sections only).
    pub fn section_at(&self, addr: u32) -> Option<&OutSection> {
        self.sections.iter().find(|s| {
            s.size > 0 && addr >= s.addr && u64::from(addr) < u64::from(s.addr) + u64::from(s.size)
        })
    }

    /// Looks up a symbol by name, preferring global ones.
    pub fn symbol(&self, name: &str) -> Option<&OutSymbol> {
        let mut found = self.symbols.iter().filter(|s| s.name == name);
        let first = found.clone().find(|s| s.bind != Bind::Local);
        first.or_else(|| found.next())
    }
}

/// A resolved global definition.
#[derive(Clone, Copy, Debug)]
pub(crate) enum GDef {
    Sym {
        obj: u32,
        sym: SymId,
        weak: bool,
    },
    Common {
        obj: u32,
        sym: SymId,
        size: u64,
        align: u64,
    },
    Layout(i64),
}

struct Linker<'a> {
    objects: &'a [Object],
    globals: HashMap<String, GDef>,
    /// Address of every input section; `None` if discarded or not placed.
    sec_addr: Vec<Vec<Option<u64>>>,
    /// Output section index of every input section.
    sec_out: Vec<Vec<Option<usize>>>,
    /// Input sections removed by `--gc-sections`.
    removed: Vec<Vec<bool>>,
    defines: &'a [(String, i64)],
}

struct LinkEnv<'l, 'a> {
    l: &'l Linker<'a>,
    obj: u32,
}

impl Env for LinkEnv<'_, '_> {
    fn symbol(&self, id: SymId, depth: u32) -> Result<Option<Value>, String> {
        self.l.sym_value(self.obj, id, depth).map(Some)
    }

    fn section_addr(&self, sec: SecRef) -> Option<i64> {
        self.l.addr_of(sec).map(|a| a as i64)
    }
}

impl<'a> Linker<'a> {
    fn addr_of(&self, s: SecRef) -> Option<u64> {
        self.sec_addr
            .get(s.obj as usize)?
            .get(s.sec as usize)
            .copied()
            .flatten()
    }

    fn section(&self, s: SecRef) -> &Section {
        &self.objects[s.obj as usize].sections[s.sec as usize]
    }

    /// Value of symbol `id` as referenced from object `obj`.
    fn sym_value(&self, obj: u32, id: SymId, depth: u32) -> Result<Value, String> {
        if depth > expr::MAX_DEPTH {
            return Err("symbol definitions are circular or nested too deeply".to_string());
        }
        let s = &self.objects[obj as usize].symbols[id as usize];
        if s.bind != Bind::Local || !s.is_defined() {
            if let Some(g) = self.globals.get(&s.name) {
                return self.global_value(*g, depth);
            }
            if !s.is_defined() {
                if s.bind == Bind::Weak {
                    return Ok(Value::Abs(0));
                }
                // A section name refers to the start of that section of this file.
                if let Some(sec) = self.objects[obj as usize].find_section(&s.name) {
                    return Ok(Value::Rel(SecRef { obj, sec }, 0));
                }
                return Err(format!("undefined symbol '{}'", s.display_name()));
            }
        }
        self.local_value(obj, s, depth)
    }

    fn global_value(&self, g: GDef, depth: u32) -> Result<Value, String> {
        match g {
            GDef::Sym { obj, sym, .. } | GDef::Common { obj, sym, .. } => self.local_value(
                obj,
                &self.objects[obj as usize].symbols[sym as usize],
                depth,
            ),
            GDef::Layout(v) => Ok(Value::Abs(v)),
        }
    }

    fn local_value(&self, obj: u32, s: &Symbol, depth: u32) -> Result<Value, String> {
        match &s.def {
            SymDef::Label { sec, off } => Ok(Value::Rel(SecRef { obj, sec: *sec }, *off as i64)),
            SymDef::Equ(e) => match expr::eval(e, &LinkEnv { l: self, obj }, depth + 1)? {
                Some(v) => Ok(v),
                None => Err(format!(
                    "cannot evaluate the value of '{}'",
                    s.display_name()
                )),
            },
            SymDef::Common { .. } => Err(format!("common symbol '{}' was not allocated", s.name)),
            SymDef::Undef => Err(format!("undefined symbol '{}'", s.display_name())),
        }
    }

    /// Converts a value to a number; `true` if it was relative to a section.
    fn number(&self, v: Value) -> Result<(i64, bool), String> {
        match v {
            Value::Abs(x) => Ok((x, false)),
            Value::Rel(s, off) => {
                match self.addr_of(s) {
                    Some(a) => Ok((a as i64 + off, true)),
                    None => {
                        let sec = self.section(s);
                        if sec.discarded {
                            Err(format!("reference to discarded section '{}'", sec.name))
                        } else {
                            Err(format!("reference to section '{}' of '{}', which the layout does not place", sec.name, self.objects[s.obj as usize].path))
                        }
                    }
                }
            }
        }
    }

    /// Evaluates a fixup expression of object `obj` to a number.
    fn eval_num(&self, obj: u32, e: &crate::expr::Expr) -> Result<(i64, bool), String> {
        match expr::eval(e, &LinkEnv { l: self, obj }, 0)? {
            Some(v) => self.number(v),
            None => Err("expression cannot be evaluated".to_string()),
        }
    }

    /// Evaluates a layout `define` expression.
    fn eval_layout(&self, e: &LExpr, layout: &Layout, outs: &[OutSection]) -> Result<i64, String> {
        Ok(match e {
            LExpr::Num(v) => *v,
            LExpr::Sym(name) => {
                if let Some(g) = self.globals.get(name) {
                    self.number(self.global_value(*g, 0)?)?.0
                } else if let Some((_, v)) = self.defines.iter().find(|(n, _)| n == name) {
                    *v
                } else {
                    return Err(format!("undefined symbol '{name}'"));
                }
            }
            LExpr::Query(q, name) => match q.as_str() {
                "ORIGIN" | "LENGTH" => {
                    let r = layout
                        .regions
                        .iter()
                        .find(|r| &r.name == name)
                        .ok_or(format!("unknown region '{name}'"))?;
                    (if q == "ORIGIN" { r.origin } else { r.length }) as i64
                }
                _ => {
                    let o = outs
                        .iter()
                        .find(|o| &o.name == name)
                        .ok_or(format!("unknown output section '{name}'"))?;
                    match q.as_str() {
                        "START" => i64::from(o.addr),
                        "END" => i64::from(o.addr) + i64::from(o.size),
                        _ => i64::from(o.size),
                    }
                }
            },
            LExpr::Align(a, n) => align_up_i64(
                self.eval_layout(a, layout, outs)?,
                self.eval_layout(n, layout, outs)?,
            )?,
            LExpr::Neg(a) => self.eval_layout(a, layout, outs)?.wrapping_neg(),
            LExpr::Bin(op, a, b) => bin(
                *op,
                self.eval_layout(a, layout, outs)?,
                self.eval_layout(b, layout, outs)?,
            )?,
        })
    }
}

/// Builds the global symbol table. Returns it and the names of global
/// commons in order of first appearance.
fn collect_globals(objects: &[Object], diags: &mut Diags) -> (HashMap<String, GDef>, Vec<String>) {
    let mut globals: HashMap<String, GDef> = HashMap::new();
    let mut commons = Vec::new();
    for (oi, obj) in objects.iter().enumerate() {
        for (si, s) in obj.symbols.iter().enumerate() {
            if s.bind == Bind::Local || !s.is_defined() || s.cmdline {
                continue;
            }
            let (oi, si) = (oi as u32, si as SymId);
            let new = match s.def {
                SymDef::Common { size, align } => GDef::Common {
                    obj: oi,
                    sym: si,
                    size,
                    align,
                },
                _ => GDef::Sym {
                    obj: oi,
                    sym: si,
                    weak: s.bind == Bind::Weak,
                },
            };
            let Some(old) = globals.get(&s.name).copied() else {
                if matches!(new, GDef::Common { .. }) {
                    commons.push(s.name.clone());
                }
                globals.insert(s.name.clone(), new);
                continue;
            };
            let replace = match (old, new) {
                (
                    GDef::Sym {
                        weak: false,
                        obj,
                        sym,
                    },
                    GDef::Sym { weak: false, .. },
                ) => {
                    let prev = objects[obj as usize].symbols[sym as usize].def_loc;
                    match s.def_loc {
                        Some(loc) => {
                            diags.error(loc, format!("multiple definition of '{}'", s.name))
                        }
                        None => diags.error_global(format!("multiple definition of '{}'", s.name)),
                    }
                    diags.note(prev, "previous definition is here");
                    false
                }
                (GDef::Sym { weak: true, .. }, GDef::Sym { weak: false, .. }) => true,
                (GDef::Common { .. }, GDef::Sym { .. }) => true,
                (
                    GDef::Common {
                        obj,
                        sym,
                        size: s0,
                        align: a0,
                    },
                    GDef::Common {
                        size: s1,
                        align: a1,
                        ..
                    },
                ) => {
                    globals.insert(
                        s.name.clone(),
                        GDef::Common {
                            obj,
                            sym,
                            size: s0.max(s1),
                            align: a0.max(a1),
                        },
                    );
                    false
                }
                _ => false,
            };
            if replace {
                globals.insert(s.name.clone(), new);
            }
        }
    }
    commons.retain(|n| matches!(globals.get(n), Some(GDef::Common { .. })));
    (globals, commons)
}

/// Links `objects` (one per input file, in command-line order).
///
/// Returns `None` if any error was reported to `diags`.
pub fn link(
    mut objects: Vec<Object>,
    layout: &Layout,
    opts: &LinkOptions,
    diags: &mut Diags,
) -> Option<Program> {
    let n_inputs = objects.len();
    let (mut globals, commons) = collect_globals(&objects, diags);
    let common_origin = add_common_object(&mut objects, &mut globals, &commons);
    let matches = match_sections(&objects, layout, n_inputs);
    let live = if opts.gc_sections {
        let roots = crate::gc::Roots {
            layout,
            matches: &matches,
            keep_symbols: &opts.keep_symbols,
        };
        crate::gc::live_sections(&objects, &globals, &roots, diags)
    } else {
        objects
            .iter()
            .map(|o| o.sections.iter().map(|s| !s.discarded).collect())
            .collect()
    };
    let (removed, gc_removed) = removed_sections(&objects, &live);
    let Placement {
        sec_addr,
        sec_out,
        mut outs,
    } = place_sections(&objects, layout, &matches, &live, diags);
    check_overlaps(&outs, diags);
    if diags.has_errors() {
        return None;
    }
    let mut linker = Linker {
        objects: &objects,
        globals,
        sec_addr,
        sec_out,
        removed,
        defines: &opts.defines,
    };
    linker.define_layout_symbols(layout, &outs, diags);
    linker.apply_fixups(&mut outs, diags);
    let entry = linker.entry(layout, &outs, diags);
    let symbols = linker.output_symbols(layout, n_inputs, &common_origin, diags);
    let (lines, asm_lines) = linker.debug_rows();
    let listing = linker.listing(n_inputs, &outs);
    let regions = region_usage(layout, &outs);
    let inputs = objects
        .iter()
        .take(n_inputs)
        .map(|o| InputFile {
            path: o.path.clone(),
            file: o.file,
        })
        .collect();
    if diags.has_errors() {
        return None;
    }
    Some(Program {
        sections: outs,
        symbols,
        entry,
        lines,
        asm_lines,
        inputs,
        listing,
        regions,
        gc_removed,
    })
}

/// Which sections `--gc-sections` removed (per object), and the non-empty
/// ones for the map.
fn removed_sections(
    objects: &[Object],
    live: &[Vec<bool>],
) -> (Vec<Vec<bool>>, Vec<RemovedSection>) {
    let mut flags = Vec::new();
    let mut list = Vec::new();
    for (obj, live) in objects.iter().zip(live) {
        let mut per_obj = Vec::new();
        for (sec, &is_live) in obj.sections.iter().zip(live) {
            let removed = !sec.discarded && !is_live;
            if removed && sec.size > 0 {
                list.push(RemovedSection {
                    path: obj.path.clone(),
                    name: sec.name.clone(),
                    size: sec.size,
                });
            }
            per_obj.push(removed);
        }
        flags.push(per_obj);
    }
    (flags, list)
}

/// Turns the merged global commons into sections of a synthetic object
/// (appended to `objects`) so they are placed like any other input section
/// by the pattern `COMMON`. Returns, per common symbol, the index of the
/// input file that first declared it.
fn add_common_object(
    objects: &mut Vec<Object>,
    globals: &mut HashMap<String, GDef>,
    commons: &[String],
) -> Vec<usize> {
    let index = objects.len() as u32;
    let mut common_obj = Object::new("COMMON", FileId::default());
    let mut origin = Vec::new();
    for name in commons {
        let Some(GDef::Common {
            obj,
            sym,
            size,
            align,
        }) = globals.get(name).copied()
        else {
            continue;
        };
        // Named after the symbol so maps can tell common symbols apart; only
        // the `COMMON` pattern matches sections of this object.
        let mut sec = Section::new(name, SecFlags::parse("aw"), true, Loc::default());
        sec.size = size;
        sec.align = align.max(1);
        common_obj.sections.push(sec);
        let id = common_obj.symbol_id(name);
        let def_loc = objects[obj as usize].symbols[sym as usize].def_loc;
        let s = &mut common_obj.symbols[id as usize];
        s.def = SymDef::Label {
            sec: (common_obj.sections.len() - 1) as u32,
            off: 0,
        };
        s.bind = Bind::Global;
        s.kind = SymKind::Object;
        s.def_loc = def_loc;
        s.size = Some((
            crate::expr::Expr::Num(size as i64),
            def_loc.unwrap_or_default(),
        ));
        origin.push(obj as usize);
        globals.insert(
            name.clone(),
            GDef::Sym {
                obj: index,
                sym: id,
                weak: false,
            },
        );
    }
    objects.push(common_obj);
    origin
}

/// Addresses of input sections and the resulting output sections.
struct Placement {
    sec_addr: Vec<Vec<Option<u64>>>,
    sec_out: Vec<Vec<Option<usize>>>,
    outs: Vec<OutSection>,
}

/// The first output rule and pattern that match each input section
/// (`None` for discarded or unmatched sections). The synthetic object of
/// common symbols (index `n_inputs`) is matched only by `COMMON`.
fn match_sections(
    objects: &[Object],
    layout: &Layout,
    n_inputs: usize,
) -> Vec<Vec<Option<(usize, usize)>>> {
    objects
        .iter()
        .enumerate()
        .map(|(oi, obj)| {
            let is_common = oi == n_inputs;
            obj.sections
                .iter()
                .map(|sec| {
                    if sec.discarded {
                        return None;
                    }
                    let matches = |p: &Pattern| {
                        if is_common {
                            p.glob == "COMMON"
                        } else {
                            p.glob != "COMMON" && glob_match(&p.glob, &sec.name)
                        }
                    };
                    layout.outputs.iter().enumerate().find_map(|(ri, rule)| {
                        rule.patterns.iter().position(matches).map(|pi| (ri, pi))
                    })
                })
                .collect()
        })
        .collect()
}

/// Places the live input sections: each goes to the output rule it matched,
/// ordered by (pattern, file, section); addresses are assigned region by
/// region.
fn place_sections(
    objects: &[Object],
    layout: &Layout,
    matches: &[Vec<Option<(usize, usize)>>],
    live: &[Vec<bool>],
    diags: &mut Diags,
) -> Placement {
    let mut sec_addr: Vec<Vec<Option<u64>>> = objects
        .iter()
        .map(|o| vec![None; o.sections.len()])
        .collect();
    let mut sec_out: Vec<Vec<Option<usize>>> = objects
        .iter()
        .map(|o| vec![None; o.sections.len()])
        .collect();
    let mut members: Vec<Vec<(usize, u32, u32)>> = vec![Vec::new(); layout.outputs.len()];
    let common_obj = objects.len() - 1;
    for (oi, obj) in objects.iter().enumerate() {
        let is_common = oi == common_obj;
        for (si, sec) in obj.sections.iter().enumerate() {
            if !live[oi][si] {
                continue;
            }
            match matches[oi][si] {
                Some((ri, pi)) => members[ri].push((pi, oi as u32, si as u32)),
                None if sec.size > 0 => {
                    let msg = if is_common {
                        "common symbols exist but no output section lists COMMON".to_string()
                    } else {
                        format!(
                            "section '{}' ({} bytes) matches no output section of the layout",
                            sec.name, sec.size
                        )
                    };
                    diags.error(sec.loc, msg);
                }
                None => {}
            }
        }
    }

    let section = |oi: u32, si: u32| &objects[oi as usize].sections[si as usize];
    let mut cursor: Vec<u64> = layout.regions.iter().map(|r| r.origin).collect();
    let mut outs = Vec::new();
    for (ri, rule) in layout.outputs.iter().enumerate() {
        let list = &mut members[ri];
        list.sort_unstable();
        let region = &layout.regions[rule.region];
        let in_align = list
            .iter()
            .map(|&(_, o, s)| section(o, s).align)
            .max()
            .unwrap_or(1);
        let align = rule.align.max(in_align).max(1);
        let start = rule.at.unwrap_or(cursor[rule.region]).div_ceil(align) * align;
        let mut addr = start;
        let mut placed = Vec::new();
        let mut flags = SecFlags::default();
        for &(_, oi, si) in list.iter() {
            let sec = section(oi, si);
            addr = addr.div_ceil(sec.align) * sec.align;
            sec_addr[oi as usize][si as usize] = Some(addr);
            sec_out[oi as usize][si as usize] = Some(ri);
            placed.push(Placed {
                obj: oi,
                sec: si,
                name: sec.name.clone(),
                addr: addr as u32,
                size: sec.size as u32,
                align: sec.align,
            });
            flags = flags.union(sec.flags);
            if rule.nobits && !sec.nobits && sec.size > 0 {
                let path = &objects[oi as usize].path;
                diags.error(
                    sec.loc,
                    format!(
                        "section '{}' of '{path}' has contents but output section '{}' is nobits",
                        sec.name, rule.name
                    ),
                );
            }
            addr += sec.size;
        }
        let end = addr;
        if end > start {
            if start < region.origin {
                diags.error(
                    rule.loc,
                    format!(
                        "output section '{}' starts below region '{}'",
                        rule.name, region.name
                    ),
                );
            } else if end > region.origin + region.length {
                let over = end - (region.origin + region.length);
                diags.error(
                    rule.loc,
                    format!(
                        "output section '{}' overflows region '{}' by {over} bytes",
                        rule.name, region.name
                    ),
                );
            }
        }
        cursor[rule.region] = end;
        // Merge/strings properties survive only if every input has them.
        let every = |f: fn(&SecFlags) -> bool| {
            !list.is_empty() && list.iter().all(|&(_, o, s)| f(&section(o, s).flags))
        };
        flags.alloc = true;
        flags.merge = every(|f| f.merge);
        flags.strings = every(|f| f.strings);
        // An output section is nobits if the rule says so or all inputs are.
        let nobits =
            rule.nobits || (!list.is_empty() && list.iter().all(|&(_, o, s)| section(o, s).nobits));
        let mut data = Vec::new();
        if !nobits && end - start > crate::asm::MAX_SECTION_BYTES {
            diags.error(
                rule.loc,
                format!(
                    "output section '{}' is larger than {} bytes",
                    rule.name,
                    crate::asm::MAX_SECTION_BYTES
                ),
            );
        } else if !nobits && end <= 1 << 32 {
            data = vec![0u8; (end - start) as usize];
            for p in placed.iter().filter(|p| !section(p.obj, p.sec).nobits) {
                let bytes = &section(p.obj, p.sec).data;
                let at = (u64::from(p.addr) - start) as usize;
                data[at..at + bytes.len()].copy_from_slice(bytes);
            }
        }
        outs.push(OutSection {
            name: rule.name.clone(),
            region: region.name.clone(),
            addr: start as u32,
            size: (end - start) as u32,
            align,
            flags,
            nobits,
            data,
            inputs: placed,
        });
    }
    Placement {
        sec_addr,
        sec_out,
        outs,
    }
}

/// Reports non-empty output sections whose address ranges overlap.
fn check_overlaps(outs: &[OutSection], diags: &mut Diags) {
    let mut spans: Vec<(u64, u64, &str)> = outs
        .iter()
        .filter(|o| o.size > 0)
        .map(|o| {
            (
                u64::from(o.addr),
                u64::from(o.addr) + u64::from(o.size),
                o.name.as_str(),
            )
        })
        .collect();
    spans.sort_unstable();
    for w in spans.windows(2) {
        if w[0].1 > w[1].0 {
            diags.error_global(format!(
                "output sections '{}' and '{}' overlap",
                w[0].2, w[1].2
            ));
        }
    }
}

/// Bytes used in each memory region.
fn region_usage(layout: &Layout, outs: &[OutSection]) -> Vec<RegionUse> {
    layout
        .regions
        .iter()
        .enumerate()
        .map(|(i, r)| RegionUse {
            name: r.name.clone(),
            origin: r.origin,
            length: r.length,
            used: outs
                .iter()
                .zip(&layout.outputs)
                .filter(|(o, rule)| rule.region == i && o.size > 0)
                .map(|(o, _)| u64::from(o.size))
                .sum(),
        })
        .collect()
}

impl Linker<'_> {
    /// Evaluates the layout's `define`s (in order) into global symbols.
    fn define_layout_symbols(&mut self, layout: &Layout, outs: &[OutSection], diags: &mut Diags) {
        for d in &layout.defines {
            match self.eval_layout(&d.expr, layout, outs) {
                Ok(v) => {
                    if let Some(GDef::Sym { obj, sym, .. }) = self.globals.get(&d.name) {
                        let prev = self.objects[*obj as usize].symbols[*sym as usize].def_loc;
                        diags.error(
                            d.loc,
                            format!(
                                "symbol '{}' is defined by the layout and by an input file",
                                d.name
                            ),
                        );
                        diags.note(prev, "input definition is here");
                    } else {
                        self.globals.insert(d.name.clone(), GDef::Layout(v));
                    }
                }
                Err(m) => diags.error(d.loc, m),
            }
        }
    }

    /// Applies every fixup and copies the patched bytes into `outs`.
    fn apply_fixups(&self, outs: &mut [OutSection], diags: &mut Diags) {
        for (oi, obj) in self.objects.iter().enumerate() {
            for (si, sec) in obj.sections.iter().enumerate() {
                let (Some(base), Some(ri)) = (self.sec_addr[oi][si], self.sec_out[oi][si]) else {
                    continue;
                };
                if sec.fixups.is_empty() {
                    continue;
                }
                let mut data = sec.data.clone();
                let this = SecRef {
                    obj: oi as u32,
                    sec: si as u32,
                };
                for f in &sec.fixups {
                    if let Err(m) = apply_fixup(self, this, base, f, &mut data) {
                        diags.error(f.loc, m);
                    }
                }
                let out = &mut outs[ri];
                if !out.nobits {
                    let at = (base - u64::from(out.addr)) as usize;
                    out.data[at..at + data.len()].copy_from_slice(&data);
                }
            }
        }
    }

    /// The entry point: the layout's `entry` symbol, else `_start`, else the
    /// start of the first output section.
    fn entry(&self, layout: &Layout, outs: &[OutSection], diags: &mut Diags) -> u32 {
        let value_of = |name: &str| {
            self.globals
                .get(name)
                .copied()
                .map(|g| self.global_value(g, 0).and_then(|v| self.number(v)))
        };
        match &layout.entry {
            Some((name, loc)) => match value_of(name) {
                Some(Ok((v, _))) => v as u32,
                Some(Err(m)) => {
                    diags.error(*loc, m);
                    0
                }
                None => {
                    diags.error(
                        *loc,
                        format!("entry symbol '{name}' is not defined (or not global)"),
                    );
                    0
                }
            },
            None => match value_of("_start") {
                Some(Ok((v, _))) => v as u32,
                _ => outs.first().map_or(0, |o| o.addr),
            },
        }
    }

    /// All symbols of the program: every defined, non-temporary symbol (only
    /// the winning definition of a global) plus the layout's definitions.
    fn output_symbols(
        &self,
        layout: &Layout,
        n_inputs: usize,
        common_origin: &[usize],
        diags: &mut Diags,
    ) -> Vec<OutSymbol> {
        let mut symbols = Vec::new();
        for (oi, obj) in self.objects.iter().enumerate() {
            for (si, s) in obj.symbols.iter().enumerate() {
                if s.temp || s.cmdline || !s.is_defined() || matches!(s.def, SymDef::Common { .. })
                {
                    continue;
                }
                if s.bind != Bind::Local {
                    match self.globals.get(&s.name) {
                        Some(GDef::Sym { obj, sym, .. })
                            if *obj as usize == oi && *sym as usize == si => {}
                        _ => continue,
                    }
                }
                let value = match self.local_value(oi as u32, s, 0) {
                    Ok(v) => v,
                    // A `.set` symbol whose operands are missing (for example
                    // because --gc-sections removed them) is simply not
                    // output; any use of it reports the error.
                    Err(_) if matches!(s.def, SymDef::Equ(_)) => continue,
                    Err(m) => {
                        diags.error(s.def_loc.unwrap_or_default(), m);
                        continue;
                    }
                };
                let (section, addr) = match value {
                    Value::Abs(v) => (None, v),
                    // Symbols of discarded or unplaced sections are dropped.
                    Value::Rel(sr, off) => match (
                        self.sec_out[sr.obj as usize][sr.sec as usize],
                        self.addr_of(sr),
                    ) {
                        (Some(o), Some(a)) => (Some(o), a as i64 + off),
                        _ => continue,
                    },
                };
                let size = match &s.size {
                    Some((e, loc)) => match self.eval_num(oi as u32, e) {
                        Ok((v, false)) => v as u32,
                        Ok((_, true)) => {
                            diags.error(*loc, "symbol size must be absolute");
                            0
                        }
                        Err(m) => {
                            diags.error(*loc, m);
                            0
                        }
                    },
                    None => 0,
                };
                // An untyped alias (`x = y + 4`) takes the type of the aliased symbol.
                let kind = match (s.kind, s.alias) {
                    (SymKind::NoType, Some(a)) => obj.symbols[a as usize].kind,
                    (k, _) => k,
                };
                let file = if oi < n_inputs {
                    Some(oi)
                } else {
                    common_origin.get(si).copied()
                };
                symbols.push(OutSymbol {
                    name: s.name.clone(),
                    addr: addr as u32,
                    size,
                    kind,
                    bind: s.bind,
                    section,
                    file,
                });
            }
        }
        for d in &layout.defines {
            if let Some(GDef::Layout(v)) = self.globals.get(&d.name) {
                symbols.push(OutSymbol {
                    name: d.name.clone(),
                    addr: *v as u32,
                    size: 0,
                    kind: SymKind::NoType,
                    bind: Bind::Global,
                    section: None,
                    file: None,
                });
            }
        }
        symbols.sort_by(|a, b| a.addr.cmp(&b.addr).then_with(|| a.name.cmp(&b.name)));
        symbols
    }

    /// `.loc` rows and per-instruction assembler lines at final addresses.
    fn debug_rows(&self) -> (Vec<LineEntry>, Vec<AsmLine>) {
        let mut lines = Vec::new();
        let mut asm_lines = Vec::new();
        for (oi, obj) in self.objects.iter().enumerate() {
            for (si, sec) in obj.sections.iter().enumerate() {
                let Some(base) = self.sec_addr[oi][si] else {
                    continue;
                };
                for &(off, loc) in &sec.insns {
                    asm_lines.push(AsmLine {
                        addr: (base + off) as u32,
                        file: loc.file,
                        line: loc.line,
                    });
                }
                for r in &sec.lines {
                    lines.push(LineEntry {
                        addr: (base + r.off) as u32,
                        file: r.file,
                        line: r.line,
                        col: r.col,
                        is_stmt: r.is_stmt,
                    });
                }
                if let Some(last) = sec.lines.last() {
                    // End of this sequence: later addresses belong to other code.
                    lines.push(LineEntry {
                        addr: (base + sec.size) as u32,
                        file: last.file,
                        line: 0,
                        col: 0,
                        is_stmt: false,
                    });
                }
            }
        }
        // At equal addresses a real row replaces an end-of-sequence marker.
        lines.sort_by_key(|l| (l.addr, l.line != 0));
        lines.dedup_by(|later, earlier| {
            let same = later.addr == earlier.addr;
            if same {
                *earlier = *later;
            }
            same
        });
        asm_lines.sort_by_key(|a| a.addr);
        (lines, asm_lines)
    }

    /// Listing lines of every input file with final addresses and bytes.
    fn listing(&self, n_inputs: usize, outs: &[OutSection]) -> Vec<Vec<ListingLine>> {
        let mut listing = Vec::new();
        for (oi, obj) in self.objects.iter().enumerate().take(n_inputs) {
            let mut out = Vec::new();
            for l in &obj.listing {
                // Statements of sections removed by --gc-sections are omitted.
                if l.place
                    .is_some_and(|(si, _, _)| self.removed[oi][si as usize])
                {
                    continue;
                }
                let placed = l.place.and_then(|(si, off, len)| {
                    let base = self.sec_addr[oi][si as usize]?;
                    let o = &outs[self.sec_out[oi][si as usize]?];
                    let a = base + off;
                    let bytes = if o.nobits || obj.sections[si as usize].nobits {
                        Vec::new()
                    } else {
                        let at = (a - u64::from(o.addr)) as usize;
                        o.data[at..at + len as usize].to_vec()
                    };
                    Some((a as u32, bytes))
                });
                let (addr, bytes) = match placed {
                    Some((a, b)) => (Some(a), b),
                    None => (None, Vec::new()),
                };
                out.push(ListingLine {
                    loc: l.loc,
                    text: l.text.clone(),
                    addr,
                    bytes,
                    code: l.code,
                });
            }
            listing.push(out);
        }
        listing
    }
}

fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

fn write_u32(data: &mut [u8], off: usize, v: u32) {
    data[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// Finds the `%pcrel_hi` fixup at `off` of section `s` and returns the
/// pc-relative distance it covers.
fn pcrel_hi_delta(l: &Linker, s: SecRef, off: u64) -> Result<i64, String> {
    let sec = l.section(s);
    let hi = sec
        .fixups
        .iter()
        .find(|f| f.off == off && f.kind == FixKind::PcrelHi20)
        .ok_or("%pcrel_lo label does not point at an auipc with a %pcrel_hi relocation")?;
    let (target, _) = l.eval_num(s.obj, &hi.expr)?;
    let pc = l.addr_of(s).ok_or("section not placed")? + off;
    Ok(i64::from((target as u32).wrapping_sub(pc as u32) as i32))
}

/// Patches the bytes of fixup `f` of section `this` (placed at `base`) into `data`.
fn apply_fixup(
    l: &Linker,
    this: SecRef,
    base: u64,
    f: &Fixup,
    data: &mut [u8],
) -> Result<(), String> {
    let obj = this.obj;
    let off = f.off as usize;
    let pc = base + f.off;
    let absolute = |what: &str| -> Result<i64, String> {
        match l.eval_num(obj, &f.expr)? {
            (v, false) => Ok(v),
            (_, true) => Err(format!(
                "{what} must be an absolute value (use %lo/%hi for addresses)"
            )),
        }
    };
    match f.kind {
        FixKind::Data(size) => {
            let (v, rel) = l.eval_num(obj, &f.expr)?;
            if rel && size != 4 {
                return Err(format!("relocatable value in {size}-byte data"));
            }
            if size < 8 {
                let bits = u32::from(size) * 8;
                if !(-(1i64 << (bits - 1))..(1i64 << bits)).contains(&v) {
                    return Err(format!("value {v:#x} does not fit in {size} byte(s)"));
                }
            }
            data[off..off + usize::from(size)]
                .copy_from_slice(&v.to_le_bytes()[..usize::from(size)]);
        }
        FixKind::ImmI | FixKind::ImmS => {
            let v = absolute("immediate")?;
            if !fits_signed(v, 12) {
                return Err(format!("immediate {v} out of range [-2048, 2047]"));
            }
            let w = read_u32(data, off);
            let w = if f.kind == FixKind::ImmI {
                with_i_imm(w, v as i32)
            } else {
                with_s_imm(w, v as i32)
            };
            write_u32(data, off, w);
        }
        FixKind::Shamt => {
            let v = absolute("shift amount")?;
            if !(0..32).contains(&v) {
                return Err(format!("shift amount {v} out of range [0, 31]"));
            }
            write_u32(data, off, read_u32(data, off) | (v as u32) << 20);
        }
        FixKind::Imm20 => {
            let v = absolute("immediate")?;
            if !(0..1 << 20).contains(&v) {
                return Err(format!("immediate {v} out of range [0, 1048575]"));
            }
            write_u32(data, off, with_u_imm(read_u32(data, off), (v << 12) as i32));
        }
        FixKind::Hi20 => {
            let (v, _) = l.eval_num(obj, &f.expr)?;
            write_u32(
                data,
                off,
                with_u_imm(read_u32(data, off), (hi20(v) << 12) as i32),
            );
        }
        FixKind::Lo12I | FixKind::Lo12S => {
            let (v, _) = l.eval_num(obj, &f.expr)?;
            let w = read_u32(data, off);
            let w = if f.kind == FixKind::Lo12I {
                with_i_imm(w, lo12(v))
            } else {
                with_s_imm(w, lo12(v))
            };
            write_u32(data, off, w);
        }
        FixKind::PcrelHi20 => {
            let (target, _) = l.eval_num(obj, &f.expr)?;
            let delta = i64::from((target as u32).wrapping_sub(pc as u32) as i32);
            write_u32(
                data,
                off,
                with_u_imm(read_u32(data, off), (hi20(delta) << 12) as i32),
            );
        }
        FixKind::PcrelLo12I(hi) | FixKind::PcrelLo12S(hi) => {
            let delta = match hi {
                Some(hi_off) => pcrel_hi_delta(l, this, hi_off)?,
                None => match expr::eval(&f.expr, &LinkEnv { l, obj }, 0)? {
                    Some(Value::Rel(s, o)) if o >= 0 => pcrel_hi_delta(l, s, o as u64)?,
                    _ => {
                        return Err(
                            "%pcrel_lo requires the label of an auipc instruction".to_string()
                        )
                    }
                },
            };
            let w = read_u32(data, off);
            let w = if matches!(f.kind, FixKind::PcrelLo12I(_)) {
                with_i_imm(w, lo12(delta))
            } else {
                with_s_imm(w, lo12(delta))
            };
            write_u32(data, off, w);
        }
        FixKind::Branch | FixKind::Jal => {
            let (target, _) = l.eval_num(obj, &f.expr)?;
            let delta = i64::from((target as u32).wrapping_sub(pc as u32) as i32);
            let (lo, hi) = if f.kind == FixKind::Branch {
                (-4096, 4094)
            } else {
                (-(1 << 20), (1 << 20) - 2)
            };
            if delta % 2 != 0 {
                return Err(format!("branch target {target:#x} is not 2-byte aligned"));
            }
            if !(lo..=hi).contains(&delta) {
                let what = if f.kind == FixKind::Branch {
                    "branch"
                } else {
                    "jump"
                };
                return Err(format!(
                    "{what} target {:#x} out of range (offset {delta}, allowed [{lo}, {hi}])",
                    target as u32
                ));
            }
            let w = read_u32(data, off);
            let w = if f.kind == FixKind::Branch {
                with_b_imm(w, delta as i32)
            } else {
                with_j_imm(w, delta as i32)
            };
            write_u32(data, off, w);
        }
    }
    Ok(())
}
