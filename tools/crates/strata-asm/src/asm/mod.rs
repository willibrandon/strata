//! Assembly of one input file into an [`Object`].
//!
//! Every instruction has a fixed size (no compressed instructions, no
//! relaxation), so each statement is placed as soon as it is read. Values
//! that are not yet known (forward references, symbols of other files,
//! section addresses) are recorded as [`Fixup`]s and resolved by the linker.

mod directives;
mod insn;

use crate::diag::{Diags, FileId, FileTable, Loc};
use crate::expr::{self, Env, Expr, Parser, Resolver, SecRef, SymId, Value};
use crate::lexer::{tokenize, Punct, Tok, Token};
use crate::object::{FixKind, Fixup, LineRow, ListLine, Object, SecFlags, Section, SymDef};
use crate::source::{parse_line_marker, LineSplitter};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Settings shared by all input files of one run.
#[derive(Clone, Debug, Default)]
pub struct AsmConfig {
    /// `-I` directories for `.include` and `.incbin`.
    pub include_dirs: Vec<PathBuf>,
    /// `-D` symbols, defined as absolute values in every file.
    pub defines: Vec<(String, i64)>,
}

/// Upper bound on statements processed per input file, including macro and
/// `.rept` expansions; protects against runaway recursion.
const STATEMENT_BUDGET: u64 = 20_000_000;
/// Maximum nesting of `.include` files and of macro expansions.
const MAX_NESTING: usize = 100;
/// Largest section with contents (Strata's memories are far smaller).
pub const MAX_SECTION_BYTES: u64 = 256 << 20;

/// One statement to assemble.
#[derive(Clone, Debug)]
pub(crate) struct Stmt {
    pub text: String,
    /// Location of the statement start (the invocation, for macro expansions).
    pub loc: Loc,
    /// Token columns can be added to `loc.col` (false inside macro expansions,
    /// whose text does not appear in the source).
    pub exact_cols: bool,
}

struct FileFrame {
    dir: PathBuf,
    lines: Vec<String>,
    next: usize,
    splitter: LineSplitter,
    /// Logical file and the line number of the next physical line.
    file: FileId,
    line: u32,
    pending: VecDeque<Stmt>,
}

struct ExpFrame {
    stmts: Vec<Stmt>,
    idx: usize,
    /// Name of the macro for macro expansions (`None` for `.rept`/`.irp`).
    macro_name: Option<String>,
    /// Depth of the conditional stack when the expansion started.
    cond_depth: usize,
}

enum Frame {
    File(FileFrame),
    Expansion(ExpFrame),
}

#[derive(Clone, Copy, Debug)]
struct Cond {
    parent_active: bool,
    active: bool,
    taken: bool,
    seen_else: bool,
    loc: Loc,
}

#[derive(Clone, Debug)]
pub(crate) struct MacroParam {
    pub name: String,
    pub default: Option<String>,
    pub required: bool,
    pub vararg: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Macro {
    pub params: Vec<MacroParam>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Copy, Debug)]
struct PendingLoc {
    file: FileId,
    line: u32,
    col: u32,
    is_stmt: bool,
}

/// An operand-level error: byte column within the statement and message.
pub(crate) type OpError = (u32, String);

/// Assembles one input file.
pub struct Assembler<'a> {
    obj_index: u32,
    obj: Object,
    files: &'a mut FileTable,
    diags: &'a mut Diags,
    cfg: &'a AsmConfig,
    stack: Vec<Frame>,
    cur: u32,
    prev: u32,
    sec_stack: Vec<(u32, u32)>,
    cond: Vec<Cond>,
    macros: HashMap<String, Rc<Macro>>,
    expansion_counter: u64,
    num_labels: HashMap<u32, u32>,
    dwarf_files: HashMap<u64, FileId>,
    comp_dir: Option<String>,
    pending_loc: Option<PendingLoc>,
    /// DWARF `is_stmt` state: `.loc ... is_stmt N` changes it for the
    /// following `.loc` directives too.
    is_stmt: bool,
    option_stack: Vec<Loc>,
    budget: u64,
    ended: bool,
}

/// Assembles the file at `path` as object number `obj_index`.
pub fn assemble_file(
    obj_index: u32,
    path: &str,
    files: &mut FileTable,
    diags: &mut Diags,
    cfg: &AsmConfig,
) -> Object {
    let file = files.intern(path);
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let dir = Path::new(path)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            assemble_text(obj_index, path, &text, dir, files, diags, cfg)
        }
        Err(e) => {
            diags.error_global(format!("cannot read input file '{path}': {e}"));
            Object::new(path, file)
        }
    }
}

/// Assembles source text; `name` is used for diagnostics and debug info and
/// `dir` for resolving relative `.include`/`.incbin` paths.
pub fn assemble_text(
    obj_index: u32,
    name: &str,
    text: &str,
    dir: PathBuf,
    files: &mut FileTable,
    diags: &mut Diags,
    cfg: &AsmConfig,
) -> Object {
    let file = files.intern(name);
    let mut asm = Assembler {
        obj_index,
        obj: Object::new(name, file),
        files,
        diags,
        cfg,
        stack: Vec::new(),
        cur: 0,
        prev: 0,
        sec_stack: Vec::new(),
        cond: Vec::new(),
        macros: HashMap::new(),
        expansion_counter: 0,
        num_labels: HashMap::new(),
        dwarf_files: HashMap::new(),
        comp_dir: None,
        pending_loc: None,
        is_stmt: true,
        option_stack: Vec::new(),
        budget: STATEMENT_BUDGET,
        ended: false,
    };
    let start = Loc {
        file,
        line: 1,
        col: 0,
    };
    asm.cur = asm.get_section(".text", None, None, start);
    asm.prev = asm.cur;
    for (sym, value) in &cfg.defines {
        let id = asm.obj.symbol_id(sym);
        let s = &mut asm.obj.symbols[id as usize];
        s.def = SymDef::Equ(Expr::Num(*value));
        s.cmdline = true;
    }
    asm.push_file(file, text, dir);
    asm.run();
    asm.obj
}

impl<'a> Assembler<'a> {
    // ----- input -----------------------------------------------------------

    fn push_file(&mut self, file: FileId, text: &str, dir: PathBuf) {
        let lines = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        self.stack.push(Frame::File(FileFrame {
            dir,
            lines,
            next: 0,
            splitter: LineSplitter::new(),
            file,
            line: 1,
            pending: VecDeque::new(),
        }));
    }

    /// Returns the next statement from the innermost input, or `None` at the
    /// end of all input.
    fn next_stmt(&mut self) -> Option<Stmt> {
        loop {
            let files = &mut *self.files;
            match self.stack.last_mut()? {
                Frame::File(f) => {
                    if let Some(s) = f.pending.pop_front() {
                        return Some(s);
                    }
                    if f.next >= f.lines.len() {
                        if f.splitter.in_block_comment() {
                            let loc = Loc {
                                file: f.file,
                                line: f.line.saturating_sub(1),
                                col: 0,
                            };
                            self.diags.error(loc, "unterminated /* comment");
                        }
                        self.stack.pop();
                        continue;
                    }
                    let text = std::mem::take(&mut f.lines[f.next]);
                    f.next += 1;
                    let line = f.line;
                    f.line = f.line.saturating_add(1);
                    if !f.splitter.in_block_comment() {
                        if let Some((n, name)) = parse_line_marker(&text) {
                            f.file = files.intern(&name);
                            f.line = n;
                            continue;
                        }
                    }
                    for p in f.splitter.split(&text) {
                        f.pending.push_back(Stmt {
                            text: p.text,
                            loc: Loc {
                                file: f.file,
                                line,
                                col: p.col + 1,
                            },
                            exact_cols: true,
                        });
                    }
                }
                Frame::Expansion(e) => {
                    if e.idx < e.stmts.len() {
                        e.idx += 1;
                        return Some(e.stmts[e.idx - 1].clone());
                    }
                    let depth = e.cond_depth;
                    let is_macro = e.macro_name.is_some();
                    self.stack.pop();
                    if is_macro && self.cond.len() > depth {
                        let loc = self.cond[depth].loc;
                        self.diags
                            .error(loc, "conditional not closed within macro expansion");
                        self.cond.truncate(depth);
                    }
                }
            }
        }
    }

    /// Directory of the innermost file being read.
    fn current_dir(&self) -> PathBuf {
        self.stack
            .iter()
            .rev()
            .find_map(|f| match f {
                Frame::File(f) => Some(f.dir.clone()),
                Frame::Expansion(_) => None,
            })
            .unwrap_or_default()
    }

    fn file_depth(&self) -> usize {
        self.stack
            .iter()
            .filter(|f| matches!(f, Frame::File(_)))
            .count()
    }

    fn macro_depth(&self) -> usize {
        self.stack
            .iter()
            .filter(|f| matches!(f, Frame::Expansion(e) if e.macro_name.is_some()))
            .count()
    }

    /// Finds `name` relative to the current file, then the `-I` directories,
    /// then the working directory.
    fn resolve_path(&self, name: &str) -> Option<PathBuf> {
        let p = Path::new(name);
        if p.is_absolute() {
            return p.exists().then(|| p.to_path_buf());
        }
        let dir = self.current_dir();
        std::iter::once(dir.join(p))
            .chain(self.cfg.include_dirs.iter().map(|d| d.join(p)))
            .chain(std::iter::once(p.to_path_buf()))
            .find(|c| c.is_file())
    }

    fn run(&mut self) {
        while let Some(stmt) = self.next_stmt() {
            if self.budget == 0 {
                self.diags.error(
                    stmt.loc,
                    "too many statements (runaway macro or .rept expansion?)",
                );
                break;
            }
            self.budget -= 1;
            self.statement(&stmt);
            if self.ended {
                break;
            }
        }
        self.finish();
    }

    fn finish(&mut self) {
        for c in std::mem::take(&mut self.cond) {
            self.diags
                .error(c.loc, "unterminated conditional (missing .endif)");
        }
        if let Some(loc) = self.option_stack.last() {
            self.diags
                .warning(*loc, ".option push without matching .option pop");
        }
        // Numeric local labels referenced forward but never defined.
        for s in &self.obj.symbols {
            if s.name.contains('\u{2}') && s.def == SymDef::Undef {
                if let Some(loc) = s.ref_loc {
                    self.diags.error(
                        loc,
                        format!("undefined local label '{}f'", s.display_name()),
                    );
                }
            }
        }
        // Alignment padding in code becomes nops.
        let nop = 0x0000_0013u32.to_le_bytes();
        for sec in &mut self.obj.sections {
            if !(sec.has_insns || sec.flags.exec) || sec.nobits {
                continue;
            }
            for &(off, len) in &sec.code_pads {
                let zeros = ((4 - off % 4) % 4).min(len);
                let words = (len - zeros) / 4;
                let start = (off + zeros) as usize;
                for w in 0..words as usize {
                    sec.data[start + 4 * w..start + 4 * w + 4].copy_from_slice(&nop);
                }
            }
        }
    }

    // ----- statements --------------------------------------------------------

    fn cond_active(&self) -> bool {
        self.cond.last().is_none_or(|c| c.parent_active && c.active)
    }

    /// Location of byte column `col` (0-based) of `stmt`.
    pub(crate) fn loc_at(&self, stmt: &Stmt, col: u32) -> Loc {
        if stmt.exact_cols {
            stmt.loc.with_col(stmt.loc.col + col)
        } else {
            stmt.loc
        }
    }

    pub(crate) fn error(&mut self, stmt: &Stmt, col: u32, msg: impl Into<String>) {
        let loc = self.loc_at(stmt, col);
        self.diags.error(loc, msg);
        self.note_expansion();
    }

    pub(crate) fn warning(&mut self, stmt: &Stmt, col: u32, msg: impl Into<String>) {
        let loc = self.loc_at(stmt, col);
        self.diags.warning(loc, msg);
        self.note_expansion();
    }

    /// Adds "in expansion of macro" context to the last diagnostic.
    fn note_expansion(&mut self) {
        let innermost = self.stack.iter().rev().find_map(|f| match f {
            Frame::Expansion(e) => e.macro_name.as_deref(),
            Frame::File(_) => None,
        });
        if let Some(name) = innermost {
            let msg = format!("in expansion of macro '{name}'");
            self.diags.note(None, msg);
        }
    }

    fn statement(&mut self, stmt: &Stmt) {
        if !self.cond_active() {
            self.inactive_statement(stmt);
            return;
        }
        let toks = match tokenize(&stmt.text) {
            Ok(t) => t,
            Err(e) => return self.error(stmt, e.col, e.msg),
        };
        let start_sec = self.cur;
        let start_off = self.cur_off();
        let start_insns = self.cur_sec().insns.len();
        let mut rest: &[Token] = &toks;
        while rest.len() >= 2 && rest[1].is_punct(Punct::Colon) {
            match &rest[0].tok {
                Tok::Ident(name) => self.define_label(name, stmt, rest[0].col),
                Tok::Int(n) if (0..=i64::from(u32::MAX)).contains(n) => {
                    self.define_numeric_label(*n as u32, stmt, rest[0].col)
                }
                _ => break,
            }
            rest = &rest[2..];
        }
        if !rest.is_empty() {
            self.dispatch(stmt, rest);
        }
        if self.cur == start_sec && !self.cur_sec().discarded {
            let end = self.cur_off();
            let code = self.cur_sec().insns.len() > start_insns;
            self.obj.listing.push(ListLine {
                loc: stmt.loc,
                text: stmt.text.clone(),
                place: Some((self.cur, start_off, end.saturating_sub(start_off))),
                code,
            });
        } else if !self.obj.sections[start_sec as usize].discarded {
            self.obj.listing.push(ListLine {
                loc: stmt.loc,
                text: stmt.text.clone(),
                place: None,
                code: false,
            });
        }
    }

    fn dispatch(&mut self, stmt: &Stmt, toks: &[Token]) {
        let first = &toks[0];
        let Some(name) = first.ident() else {
            return self.error(
                stmt,
                first.col,
                format!(
                    "expected a label, directive or instruction, found {}",
                    first.tok
                ),
            );
        };
        if toks.len() >= 2 && toks[1].is_punct(Punct::Assign) {
            return self.assign(
                name,
                &toks[2..],
                stmt,
                first.col,
                directives::AssignKind::Set,
            );
        }
        let lower = name.to_ascii_lowercase();
        if lower.starts_with('.') && lower.len() > 1 {
            if self.directive(&lower, stmt, first, &toks[1..]) {
                return;
            }
            if let Some(m) = self.macros.get(name).cloned() {
                return self.expand_macro(name, m, stmt, first);
            }
            return self.error(stmt, first.col, format!("unknown directive '{name}'"));
        }
        if let Some(m) = self.macros.get(name).cloned() {
            return self.expand_macro(name, m, stmt, first);
        }
        if self.cur_sec().discarded {
            return;
        }
        self.instruction(&lower, stmt, first, &toks[1..]);
    }

    // ----- sections ------------------------------------------------------------

    pub(crate) fn cur_sec(&self) -> &Section {
        &self.obj.sections[self.cur as usize]
    }

    pub(crate) fn cur_sec_mut(&mut self) -> &mut Section {
        &mut self.obj.sections[self.cur as usize]
    }

    pub(crate) fn cur_ref(&self) -> SecRef {
        SecRef {
            obj: self.obj_index,
            sec: self.cur,
        }
    }

    pub(crate) fn cur_off(&self) -> u64 {
        self.cur_sec().size
    }

    /// Returns the section `name`, creating it with the given (or default)
    /// flags and type.
    pub(crate) fn get_section(
        &mut self,
        name: &str,
        flags: Option<SecFlags>,
        nobits: Option<bool>,
        loc: Loc,
    ) -> u32 {
        if let Some(i) = self.obj.find_section(name) {
            return i;
        }
        let flags = flags.unwrap_or_else(|| SecFlags::for_name(name));
        let nobits = nobits.unwrap_or(false) || crate::object::is_nobits_name(name);
        let mut sec = Section::new(name, flags, nobits, loc);
        sec.discarded = crate::object::is_discarded_name(name);
        self.obj.sections.push(sec);
        (self.obj.sections.len() - 1) as u32
    }

    pub(crate) fn switch_section(&mut self, idx: u32) {
        if idx != self.cur {
            self.prev = self.cur;
            self.cur = idx;
        }
    }

    // ----- symbols ----------------------------------------------------------------

    fn define_label(&mut self, name: &str, stmt: &Stmt, col: u32) {
        let loc = self.loc_at(stmt, col);
        let id = self.obj.symbol_id(name);
        let (sec, off) = (self.cur, self.cur_off());
        let sym = &mut self.obj.symbols[id as usize];
        if sym.is_defined() && !sym.cmdline {
            let prev = sym.def_loc;
            self.error(stmt, col, format!("symbol '{name}' is already defined"));
            self.diags.note(prev, "previous definition is here");
            return;
        }
        sym.def = SymDef::Label { sec, off };
        sym.cmdline = false;
        sym.def_loc = Some(loc);
    }

    fn define_numeric_label(&mut self, n: u32, stmt: &Stmt, col: u32) {
        let count = self.num_labels.entry(n).or_insert(0);
        *count += 1;
        let name = format!("{n}\u{2}{count}");
        self.define_label(&name, stmt, col);
    }

    /// Records a reference for "undefined symbol" diagnostics.
    fn note_reference(&mut self, id: SymId, loc: Loc) {
        let s = &mut self.obj.symbols[id as usize];
        if s.ref_loc.is_none() {
            s.ref_loc = Some(loc);
        }
    }

    // ----- expressions --------------------------------------------------------------

    /// Parses `toks` as one complete expression, binding symbols.
    pub(crate) fn parse_expr(&mut self, stmt: &Stmt, toks: &[Token]) -> Result<Expr, OpError> {
        let end_col = toks
            .last()
            .map_or(stmt.text.len() as u32, |t| t.col + t.len);
        let loc = self.loc_at(stmt, toks.first().map_or(0, |t| t.col));
        let mut parser = Parser::new(toks, end_col);
        let mut binder = Binder { asm: self, loc };
        let e = parser.expr(&mut binder).map_err(|e| (e.col, e.msg))?;
        if let Some(t) = parser.peek() {
            return Err((t.col, format!("unexpected {} in expression", t.tok)));
        }
        Ok(e)
    }

    /// Evaluates `e` with what is known now.
    pub(crate) fn eval_now(&self, e: &Expr) -> Result<Option<Value>, String> {
        expr::eval(
            e,
            &NowEnv {
                obj: &self.obj,
                obj_index: self.obj_index,
            },
            0,
        )
    }

    /// Evaluates `e`, which must be absolute and known now.
    pub(crate) fn known_abs(&self, e: &Expr) -> Result<i64, String> {
        match self.eval_now(e)? {
            Some(Value::Abs(v)) => Ok(v),
            Some(Value::Rel(..)) => Err("expression must be absolute".to_string()),
            None => Err("expression must be an absolute value known at this point".to_string()),
        }
    }

    /// Parses and evaluates an absolute expression known now.
    pub(crate) fn abs_operand(&mut self, stmt: &Stmt, toks: &[Token]) -> Result<i64, OpError> {
        let col = toks.first().map_or(0, |t| t.col);
        let e = self.parse_expr(stmt, toks)?;
        self.known_abs(&e).map_err(|m| (col, m))
    }

    // ----- emission ---------------------------------------------------------------------

    /// Appends bytes to the current section (zero bytes only in nobits sections).
    pub(crate) fn emit_bytes(&mut self, bytes: &[u8], stmt: &Stmt, col: u32) {
        let sec = self.cur_sec_mut();
        if sec.discarded {
            return;
        }
        if !sec.nobits && sec.size + bytes.len() as u64 > MAX_SECTION_BYTES {
            let name = sec.name.clone();
            return self.error(
                stmt,
                col,
                format!("section '{name}' would exceed {MAX_SECTION_BYTES} bytes"),
            );
        }
        if sec.nobits {
            if bytes.iter().any(|&b| b != 0) {
                let name = sec.name.clone();
                return self.error(
                    stmt,
                    col,
                    format!("non-zero data in nobits section '{name}'"),
                );
            }
            sec.size += bytes.len() as u64;
            return;
        }
        sec.data.extend_from_slice(bytes);
        sec.size = sec.data.len() as u64;
    }

    /// Appends `n` copies of `byte`.
    pub(crate) fn emit_fill(&mut self, n: u64, byte: u8, stmt: &Stmt, col: u32) {
        let sec = self.cur_sec();
        if sec.discarded {
            return;
        }
        // Sections with contents are held in memory; nobits ones only count.
        let limit = if sec.nobits {
            1 << 32
        } else {
            MAX_SECTION_BYTES
        };
        if sec.size.saturating_add(n) > limit {
            let name = sec.name.clone();
            return self.error(
                stmt,
                col,
                format!("section '{name}' would exceed {limit} bytes"),
            );
        }
        let sec = self.cur_sec_mut();
        if sec.nobits {
            if byte != 0 {
                return self.error(stmt, col, "non-zero fill in nobits section");
            }
            sec.size += n;
            return;
        }
        sec.data.resize(sec.data.len() + n as usize, byte);
        sec.size = sec.data.len() as u64;
    }

    /// Records a fixup at the current offset of the current section.
    pub(crate) fn add_fixup(&mut self, kind: FixKind, expr: Expr, loc: Loc) {
        let off = self.cur_off();
        let sec = self.cur_sec_mut();
        if !sec.discarded {
            sec.fixups.push(Fixup {
                off,
                kind,
                expr,
                loc,
            });
        }
    }

    /// Emits one instruction word with an optional fixup.
    pub(crate) fn emit_insn(&mut self, word: u32, fix: Option<(FixKind, Expr)>, stmt: &Stmt) {
        if self.cur_sec().discarded {
            return;
        }
        if self.cur_sec().nobits {
            let name = self.cur_sec().name.clone();
            return self.error(stmt, 0, format!("instruction in nobits section '{name}'"));
        }
        let off = self.cur_off();
        if let Some(p) = self.pending_loc.take() {
            self.cur_sec_mut().lines.push(LineRow {
                off,
                file: p.file,
                line: p.line,
                col: p.col,
                is_stmt: p.is_stmt,
            });
        }
        if let Some((kind, expr)) = fix {
            self.add_fixup(kind, expr, stmt.loc);
        }
        let loc = stmt.loc;
        let sec = self.cur_sec_mut();
        sec.insns.push((off, loc));
        sec.has_insns = true;
        sec.align = sec.align.max(4);
        sec.data.extend_from_slice(&word.to_le_bytes());
        sec.size = sec.data.len() as u64;
    }

    /// Aligns the current section to `align` bytes (a power of two).
    pub(crate) fn align_to(
        &mut self,
        align: u64,
        fill: Option<u8>,
        max: Option<u64>,
        stmt: &Stmt,
        col: u32,
    ) {
        let off = self.cur_off();
        let sec = self.cur_sec_mut();
        sec.align = sec.align.max(align);
        let pad = (align - off % align) % align;
        if pad == 0 || max.is_some_and(|m| pad > m) {
            return;
        }
        match fill {
            Some(b) => self.emit_fill(pad, b, stmt, col),
            None => {
                if !self.cur_sec().nobits && !self.cur_sec().discarded {
                    self.cur_sec_mut().code_pads.push((off, pad));
                }
                self.emit_fill(pad, 0, stmt, col);
            }
        }
    }
}

/// Largest `.set` definition copied into expressions that use it.
const INLINE_LIMIT: u32 = 64;

/// Binds identifiers while parsing: symbols, `.`, numeric labels. Symbols
/// defined by `.set`/`.equ` are replaced by their current definition, so a
/// later redefinition does not affect earlier uses.
struct Binder<'s, 'a> {
    asm: &'s mut Assembler<'a>,
    loc: Loc,
}

impl Resolver for Binder<'_, '_> {
    fn ident(&mut self, name: &str) -> Result<Expr, String> {
        if name == "." {
            return Ok(Expr::Loc(self.asm.cur_ref(), self.asm.cur_off() as i64));
        }
        let id = self.asm.obj.symbol_id(name);
        match &self.asm.obj.symbols[id as usize].def {
            // Large definitions are referenced instead of copied, which bounds
            // the size of expressions built by repeated `.set x, x + x`.
            SymDef::Equ(e) if e.size_up_to(INLINE_LIMIT) < INLINE_LIMIT => Ok(e.clone()),
            SymDef::Undef => {
                self.asm.note_reference(id, self.loc);
                Ok(Expr::Sym(id))
            }
            _ => Ok(Expr::Sym(id)),
        }
    }

    fn num_label(&mut self, n: u32, forward: bool) -> Result<Expr, String> {
        let count = self.asm.num_labels.get(&n).copied().unwrap_or(0);
        if !forward && count == 0 {
            return Err(format!("no previous definition of local label '{n}'"));
        }
        let instance = if forward { count + 1 } else { count };
        let id = self.asm.obj.symbol_id(&format!("{n}\u{2}{instance}"));
        self.asm.note_reference(id, self.loc);
        Ok(Expr::Sym(id))
    }
}

/// Symbol values known while assembling: labels of this object and
/// `.set` symbols; everything else is unknown until link time.
struct NowEnv<'o> {
    obj: &'o Object,
    obj_index: u32,
}

impl Env for NowEnv<'_> {
    fn symbol(&self, id: SymId, depth: u32) -> Result<Option<Value>, String> {
        match &self.obj.symbols[id as usize].def {
            SymDef::Label { sec, off } => Ok(Some(Value::Rel(
                SecRef {
                    obj: self.obj_index,
                    sec: *sec,
                },
                *off as i64,
            ))),
            SymDef::Equ(e) => expr::eval(e, self, depth),
            _ => Ok(None),
        }
    }

    fn section_addr(&self, _: SecRef) -> Option<i64> {
        None
    }
}

/// Splits tokens at top-level commas (outside parentheses).
pub(crate) fn split_commas(toks: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        match t.tok {
            Tok::Punct(Punct::LParen) => depth += 1,
            Tok::Punct(Punct::RParen) => depth -= 1,
            Tok::Punct(Punct::Comma) if depth == 0 => {
                out.push(&toks[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if !toks.is_empty() {
        out.push(&toks[start..]);
    }
    out
}
