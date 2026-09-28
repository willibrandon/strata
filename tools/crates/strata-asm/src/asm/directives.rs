//! Assembler directives.

use super::{
    split_commas, Assembler, Cond, ExpFrame, Frame, Macro, MacroParam, OpError, PendingLoc, Stmt,
    MAX_NESTING,
};
use crate::expr::{Expr, Value};
use crate::lexer::{is_ident_char, is_ident_start, Punct, Tok, Token};
use crate::object::{Bind, FixKind, SecFlags, SymDef, SymKind};
use std::rc::Rc;

/// Flavor of symbol assignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AssignKind {
    /// `.set`, `.equ`, `=`: may redefine a previous assignment.
    Set,
    /// `.equiv`: error if the symbol is already defined.
    Equiv,
}

/// Directives accepted and ignored.
const IGNORED: &[&str] = &[
    ".ident",
    ".attribute",
    ".addrsig",
    ".addrsig_sym",
    ".hidden",
    ".protected",
    ".internal",
    ".variant_cc",
    ".loc_mark_labels",
];

impl Assembler<'_> {
    /// Handles directive `name` (lowercase). Returns false if unknown.
    pub(crate) fn directive(
        &mut self,
        name: &str,
        stmt: &Stmt,
        head: &Token,
        args: &[Token],
    ) -> bool {
        let raw = raw_args(stmt, head);
        let res: Result<(), OpError> = match name {
            ".text" | ".data" | ".bss" | ".rodata" => self.dir_simple_section(name, stmt, args),
            ".section" => self.dir_section(stmt, head, raw, false),
            ".pushsection" => self.dir_section(stmt, head, raw, true),
            ".popsection" => match self.sec_stack.pop() {
                Some((cur, prev)) => {
                    self.cur = cur;
                    self.prev = prev;
                    Ok(())
                }
                None => Err((
                    head.col,
                    ".popsection without matching .pushsection".to_string(),
                )),
            },
            ".previous" => {
                std::mem::swap(&mut self.cur, &mut self.prev);
                Ok(())
            }
            ".byte" => self.dir_data(stmt, args, 1),
            ".2byte" | ".half" | ".short" | ".hword" => self.dir_data(stmt, args, 2),
            ".4byte" | ".word" | ".long" | ".int" => self.dir_data(stmt, args, 4),
            ".8byte" | ".quad" | ".dword" => self.dir_data(stmt, args, 8),
            ".uleb128" => self.dir_leb128(stmt, args, false),
            ".sleb128" => self.dir_leb128(stmt, args, true),
            ".ascii" => self.dir_ascii(stmt, head, args, false),
            ".asciz" | ".string" => self.dir_ascii(stmt, head, args, true),
            ".zero" => self.dir_space(stmt, head, args, true),
            ".space" | ".skip" => self.dir_space(stmt, head, args, false),
            ".fill" => self.dir_fill(stmt, head, args),
            ".align" | ".p2align" => self.dir_align(stmt, head, args, true),
            ".balign" => self.dir_align(stmt, head, args, false),
            ".org" => self.dir_org(stmt, head, args),
            ".globl" | ".global" => self.dir_bind(args, Bind::Global),
            ".weak" => self.dir_bind(args, Bind::Weak),
            ".local" => self.dir_bind(args, Bind::Local),
            ".type" => self.dir_type(head, args),
            ".size" => self.dir_size(stmt, head, args),
            ".equ" | ".set" => self.dir_assign(stmt, head, args, AssignKind::Set),
            ".equiv" => self.dir_assign(stmt, head, args, AssignKind::Equiv),
            ".comm" => self.dir_comm(stmt, head, args, false),
            ".lcomm" => self.dir_comm(stmt, head, args, true),
            ".incbin" => self.dir_incbin(stmt, head, args),
            ".include" => self.dir_include(head, args),
            ".macro" => self.dir_macro(stmt, head, raw),
            ".endm" | ".endmacro" => Err((head.col, format!("{name} without matching .macro"))),
            ".exitm" => self.dir_exitm(head),
            ".purgem" => {
                let n = raw.trim();
                if self.macros.remove(n).is_none() {
                    Err((head.col, format!("macro '{n}' is not defined")))
                } else {
                    Ok(())
                }
            }
            ".rept" => self.dir_rept(stmt, head, args),
            ".irp" => self.dir_irp(stmt, head, raw, false),
            ".irpc" => self.dir_irp(stmt, head, raw, true),
            ".endr" => Err((head.col, ".endr without matching .rept/.irp".to_string())),
            ".if" | ".ifdef" | ".ifndef" | ".ifnotdef" | ".ifb" | ".ifnb" | ".ifc" | ".ifnc"
            | ".ifeq" | ".ifne" | ".ifgt" | ".ifge" | ".iflt" | ".ifle" => {
                self.dir_if(name, stmt, head, args, raw)
            }
            ".elseif" => self.dir_elseif(stmt, head, args),
            ".else" => self.dir_else(head),
            ".endif" => match self.cond.pop() {
                Some(_) => Ok(()),
                None => Err((head.col, ".endif without matching .if".to_string())),
            },
            ".error" | ".err" => {
                let msg =
                    string_arg(args).unwrap_or_else(|| ".error directive invoked".to_string());
                Err((head.col, msg))
            }
            ".warning" => {
                let msg =
                    string_arg(args).unwrap_or_else(|| ".warning directive invoked".to_string());
                self.warning(stmt, head.col, msg);
                Ok(())
            }
            ".print" => {
                if let Some(msg) = string_arg(args) {
                    eprintln!("{msg}");
                }
                Ok(())
            }
            ".file" => self.dir_file(args),
            ".loc" => self.dir_loc(args, head),
            ".option" => self.dir_option(stmt, head, args),
            ".end" => {
                self.ended = true;
                Ok(())
            }
            _ if name.starts_with(".cfi_") || IGNORED.contains(&name) => Ok(()),
            _ => return false,
        };
        if let Err((col, msg)) = res {
            self.error(stmt, col, msg);
        }
        true
    }

    /// Statements inside a false conditional: only track nesting.
    pub(crate) fn inactive_statement(&mut self, stmt: &Stmt) {
        let word = first_word(&stmt.text);
        let loc = stmt.loc;
        match word.as_str() {
            w if w.starts_with(".if") => self.cond.push(Cond {
                parent_active: false,
                active: false,
                taken: true,
                seen_else: false,
                loc,
            }),
            ".else" | ".elseif" => {
                // Only a branch of a conditional whose enclosing code is active can start.
                if !self.cond.last().is_some_and(|c| c.parent_active) {
                    return;
                }
                if let Ok(toks) = crate::lexer::tokenize(&stmt.text) {
                    if let Some(head) = toks.first() {
                        let res = if word == ".else" {
                            self.dir_else(head)
                        } else {
                            self.dir_elseif(stmt, head, &toks[1..])
                        };
                        if let Err((col, msg)) = res {
                            self.error(stmt, col, msg);
                        }
                    }
                }
            }
            ".endif" => {
                self.cond.pop();
            }
            _ => {}
        }
    }

    // ----- sections -------------------------------------------------------------------

    fn dir_simple_section(
        &mut self,
        name: &str,
        stmt: &Stmt,
        args: &[Token],
    ) -> Result<(), OpError> {
        if let Some(t) = args.first() {
            return Err((t.col, "subsections are not supported".to_string()));
        }
        let idx = self.get_section(name, None, None, stmt.loc);
        self.switch_section(idx);
        Ok(())
    }

    fn dir_section(
        &mut self,
        stmt: &Stmt,
        head: &Token,
        raw: &str,
        push: bool,
    ) -> Result<(), OpError> {
        let base = head.col + head.len;
        let parts = split_raw(raw);
        let Some(&(off, name)) = parts.first() else {
            return Err((head.col, "expected section name".to_string()));
        };
        let name = unquote(name.trim());
        if name.is_empty() {
            return Err((base + off as u32, "expected section name".to_string()));
        }
        let mut flags = None;
        let mut nobits = None;
        let mut entsize = 0;
        if let Some(&(_, f)) = parts.get(1) {
            let f = f.trim();
            if f.starts_with('"') {
                flags = Some(SecFlags::parse(&unquote(f)));
            }
        }
        if let Some(&(toff, t)) = parts.get(2) {
            let t = t.trim().trim_start_matches(['@', '%']);
            let t = unquote(t);
            match t.as_str() {
                "nobits" => nobits = Some(true),
                "progbits" | "note" | "init_array" | "fini_array" | "preinit_array" | "unwind" => {
                    nobits = Some(false)
                }
                other => {
                    return Err((
                        base + toff as u32,
                        format!("unknown section type '{other}'"),
                    ))
                }
            }
        }
        if let Some(&(eoff, e)) = parts.get(3) {
            if flags.is_some_and(|f| f.merge) {
                entsize = e
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| (base + eoff as u32, "invalid entry size".to_string()))?;
            }
        }
        let idx = self.get_section(&name, flags, nobits, stmt.loc);
        if entsize != 0 {
            self.obj.sections[idx as usize].entsize = entsize;
        }
        if push {
            self.sec_stack.push((self.cur, self.prev));
        }
        self.switch_section(idx);
        Ok(())
    }

    // ----- data ---------------------------------------------------------------------------

    fn dir_data(&mut self, stmt: &Stmt, args: &[Token], size: u8) -> Result<(), OpError> {
        if self.cur_sec().discarded {
            return Ok(());
        }
        for op in split_commas(args) {
            let col = op.first().map_or(0, |t| t.col);
            if op.is_empty() {
                return Err((col, "expected an expression".to_string()));
            }
            let e = self.parse_expr(stmt, op)?;
            match self.eval_now(&e).map_err(|m| (col, m))? {
                Some(Value::Abs(v)) => {
                    check_data_range(v, size).map_err(|m| (col, m))?;
                    let bytes = v.to_le_bytes();
                    self.emit_bytes(&bytes[..usize::from(size)], stmt, col);
                }
                Some(Value::Rel(..)) if size != 4 => {
                    return Err((
                        col,
                        format!("relocatable expression not allowed in {size}-byte data"),
                    ));
                }
                _ => {
                    let loc = self.loc_at(stmt, col);
                    self.add_fixup(FixKind::Data(size), e, loc);
                    self.emit_fill(u64::from(size), 0, stmt, col);
                }
            }
        }
        Ok(())
    }

    fn dir_leb128(&mut self, stmt: &Stmt, args: &[Token], signed: bool) -> Result<(), OpError> {
        if self.cur_sec().discarded {
            return Ok(());
        }
        for op in split_commas(args) {
            let col = op.first().map_or(0, |t| t.col);
            let v = self.abs_operand(stmt, op)?;
            let mut out = Vec::new();
            let mut v = v;
            loop {
                let byte = (v & 0x7F) as u8;
                v = if signed {
                    v >> 7
                } else {
                    ((v as u64) >> 7) as i64
                };
                let done = if signed {
                    (v == 0 && byte & 0x40 == 0) || (v == -1 && byte & 0x40 != 0)
                } else {
                    v == 0
                };
                out.push(if done { byte } else { byte | 0x80 });
                if done {
                    break;
                }
            }
            self.emit_bytes(&out, stmt, col);
        }
        Ok(())
    }

    fn dir_ascii(
        &mut self,
        stmt: &Stmt,
        head: &Token,
        args: &[Token],
        nul: bool,
    ) -> Result<(), OpError> {
        if self.cur_sec().discarded {
            return Ok(());
        }
        if args.is_empty() {
            return Err((head.col, "expected a string".to_string()));
        }
        for op in split_commas(args) {
            let mut bytes = Vec::new();
            for t in op {
                match &t.tok {
                    Tok::Str(s) => bytes.extend_from_slice(s),
                    other => return Err((t.col, format!("expected a string, found {other}"))),
                }
            }
            if op.is_empty() {
                return Err((head.col, "expected a string".to_string()));
            }
            if nul {
                bytes.push(0);
            }
            self.emit_bytes(&bytes, stmt, op[0].col);
        }
        Ok(())
    }

    fn dir_space(
        &mut self,
        stmt: &Stmt,
        head: &Token,
        args: &[Token],
        zero_only: bool,
    ) -> Result<(), OpError> {
        if self.cur_sec().discarded {
            return Ok(());
        }
        let ops = split_commas(args);
        if ops.is_empty() || ops.len() > if zero_only { 1 } else { 2 } {
            return Err((head.col, "expected size[, fill]".to_string()));
        }
        let n = self.abs_operand(stmt, ops[0])?;
        if n < 0 {
            return Err((ops[0][0].col, "size must not be negative".to_string()));
        }
        let fill = match ops.get(1) {
            Some(op) => self.abs_operand(stmt, op)? as u8,
            None => 0,
        };
        self.emit_fill(n as u64, fill, stmt, head.col);
        Ok(())
    }

    fn dir_fill(&mut self, stmt: &Stmt, head: &Token, args: &[Token]) -> Result<(), OpError> {
        if self.cur_sec().discarded {
            return Ok(());
        }
        let ops = split_commas(args);
        if ops.is_empty() || ops.len() > 3 {
            return Err((head.col, "expected count[, size[, value]]".to_string()));
        }
        let count = self.abs_operand(stmt, ops[0])?;
        let size = match ops.get(1) {
            Some(op) if !op.is_empty() => self.abs_operand(stmt, op)?,
            _ => 1,
        };
        let value = match ops.get(2) {
            Some(op) if !op.is_empty() => self.abs_operand(stmt, op)?,
            _ => 0,
        };
        if count < 0 || size < 0 {
            return Err((head.col, "count and size must not be negative".to_string()));
        }
        let size = size.min(8) as usize;
        if (count as u64).saturating_mul(size as u64) > super::MAX_SECTION_BYTES {
            return Err((head.col, "fill too large".to_string()));
        }
        let item = &value.to_le_bytes()[..size];
        if item.iter().all(|&b| b == 0) {
            self.emit_fill(count as u64 * size as u64, 0, stmt, head.col);
        } else {
            let bytes: Vec<u8> = item
                .iter()
                .copied()
                .cycle()
                .take(count as usize * size)
                .collect();
            self.emit_bytes(&bytes, stmt, head.col);
        }
        Ok(())
    }

    fn dir_align(
        &mut self,
        stmt: &Stmt,
        head: &Token,
        args: &[Token],
        pow2: bool,
    ) -> Result<(), OpError> {
        let ops = split_commas(args);
        if ops.is_empty() || ops[0].is_empty() || ops.len() > 3 {
            return Err((head.col, "expected alignment[, fill[, max]]".to_string()));
        }
        let col = ops[0][0].col;
        let n = self.abs_operand(stmt, ops[0])?;
        let align = if pow2 {
            if !(0..=30).contains(&n) {
                return Err((
                    col,
                    "alignment exponent must be between 0 and 30".to_string(),
                ));
            }
            1u64 << n
        } else {
            if n <= 0 || n > 1 << 30 || n & (n - 1) != 0 {
                return Err((col, "alignment must be a power of two".to_string()));
            }
            n as u64
        };
        let fill = match ops.get(1) {
            Some(op) if !op.is_empty() => {
                let v = self.abs_operand(stmt, op)?;
                if !(-128..=255).contains(&v) {
                    return Err((op[0].col, "fill value must fit in a byte".to_string()));
                }
                Some(v as u8)
            }
            _ => None,
        };
        let max = match ops.get(2) {
            Some(op) if !op.is_empty() => {
                let v = self.abs_operand(stmt, op)?;
                if v < 0 {
                    return Err((op[0].col, "maximum must not be negative".to_string()));
                }
                Some(v as u64)
            }
            _ => None,
        };
        self.align_to(align, fill, max, stmt, head.col);
        Ok(())
    }

    fn dir_org(&mut self, stmt: &Stmt, head: &Token, args: &[Token]) -> Result<(), OpError> {
        let ops = split_commas(args);
        if ops.is_empty() || ops.len() > 2 {
            return Err((head.col, "expected offset[, fill]".to_string()));
        }
        let col = ops[0].first().map_or(head.col, |t| t.col);
        let e = self.parse_expr(stmt, ops[0])?;
        let target = match self.eval_now(&e).map_err(|m| (col, m))? {
            Some(Value::Abs(v)) => v,
            Some(Value::Rel(s, v)) if s == self.cur_ref() => v,
            _ => {
                return Err((
                    col,
                    ".org requires an absolute value or a location in the current section"
                        .to_string(),
                ))
            }
        };
        let fill = match ops.get(1) {
            Some(op) => self.abs_operand(stmt, op)? as u8,
            None => 0,
        };
        let off = self.cur_off() as i64;
        if target < off {
            return Err((
                col,
                "cannot move the location counter backwards".to_string(),
            ));
        }
        self.emit_fill((target - off) as u64, fill, stmt, col);
        Ok(())
    }

    // ----- symbols -------------------------------------------------------------------------

    fn symbol_names<'t>(&self, args: &'t [Token]) -> Result<Vec<(&'t str, u32)>, OpError> {
        let mut out = Vec::new();
        for op in split_commas(args) {
            match op {
                [t] if t.ident().is_some() => out.push((t.ident().unwrap_or_default(), t.col)),
                [] => return Err((0, "expected a symbol name".to_string())),
                [t, ..] => return Err((t.col, "expected a symbol name".to_string())),
            }
        }
        Ok(out)
    }

    fn dir_bind(&mut self, args: &[Token], bind: Bind) -> Result<(), OpError> {
        for (name, _) in self.symbol_names(args)? {
            let id = self.obj.symbol_id(name);
            let sym = &mut self.obj.symbols[id as usize];
            sym.bind = bind;
            sym.local_directive = bind == Bind::Local;
        }
        Ok(())
    }

    fn dir_type(&mut self, head: &Token, args: &[Token]) -> Result<(), OpError> {
        let ops = split_commas(args);
        let (Some(sym_op), Some(ty_op)) = (ops.first(), ops.get(1)) else {
            return Err((head.col, "expected symbol, type".to_string()));
        };
        let [sym_tok] = sym_op else {
            return Err((head.col, "expected a symbol name".to_string()));
        };
        let Some(name) = sym_tok.ident() else {
            return Err((sym_tok.col, "expected a symbol name".to_string()));
        };
        let ty = ty_op
            .iter()
            .rev()
            .find_map(|t| match &t.tok {
                Tok::Ident(s) => Some(s.clone()),
                Tok::Str(s) => Some(String::from_utf8_lossy(s).into_owned()),
                _ => None,
            })
            .unwrap_or_default();
        let kind = match ty.to_ascii_lowercase().as_str() {
            "function" | "stt_func" => SymKind::Func,
            "object" | "stt_object" => SymKind::Object,
            "notype" | "stt_notype" => SymKind::NoType,
            _ => {
                let col = ty_op.first().map_or(head.col, |t| t.col);
                return Err((col, format!("unsupported symbol type '{ty}'")));
            }
        };
        let id = self.obj.symbol_id(name);
        self.obj.symbols[id as usize].kind = kind;
        Ok(())
    }

    fn dir_size(&mut self, stmt: &Stmt, head: &Token, args: &[Token]) -> Result<(), OpError> {
        let ops = split_commas(args);
        let (Some([sym_tok]), Some(size_op)) = (ops.first().copied(), ops.get(1)) else {
            return Err((head.col, "expected symbol, size".to_string()));
        };
        let Some(name) = sym_tok.ident() else {
            return Err((sym_tok.col, "expected a symbol name".to_string()));
        };
        let e = self.parse_expr(stmt, size_op)?;
        let loc = self.loc_at(stmt, size_op.first().map_or(head.col, |t| t.col));
        let id = self.obj.symbol_id(name);
        self.obj.symbols[id as usize].size = Some((e, loc));
        Ok(())
    }

    fn dir_assign(
        &mut self,
        stmt: &Stmt,
        head: &Token,
        args: &[Token],
        kind: AssignKind,
    ) -> Result<(), OpError> {
        let Some(comma) = args.iter().position(|t| t.is_punct(Punct::Comma)) else {
            return Err((head.col, "expected symbol, expression".to_string()));
        };
        let [sym_tok] = &args[..comma] else {
            return Err((head.col, "expected a symbol name".to_string()));
        };
        let Some(name) = sym_tok.ident() else {
            return Err((sym_tok.col, "expected a symbol name".to_string()));
        };
        self.assign(name, &args[comma + 1..], stmt, sym_tok.col, kind);
        Ok(())
    }

    /// `name = expr` and friends.
    pub(crate) fn assign(
        &mut self,
        name: &str,
        value: &[Token],
        stmt: &Stmt,
        col: u32,
        kind: AssignKind,
    ) {
        if name == "." {
            return self.error(
                stmt,
                col,
                "assignment to '.' is not supported; use .org or .space",
            );
        }
        let e = match self.parse_expr(stmt, value) {
            Ok(e) => e,
            Err((c, m)) => return self.error(stmt, c, m),
        };
        let vcol = value.first().map_or(col, |t| t.col);
        let alias = match &e {
            Expr::Sym(id) => Some(*id),
            Expr::Binary(crate::expr::BinOp::Add | crate::expr::BinOp::Sub, a, _) => {
                match a.as_ref() {
                    Expr::Sym(id) => Some(*id),
                    _ => None,
                }
            }
            _ => None,
        };
        // Fold what is already known so that later changes to the operands
        // (e.g. `.set x, x + 1`) do not affect this definition.
        let folded = match self.eval_now(&e) {
            Ok(Some(Value::Abs(v))) => Expr::Num(v),
            Ok(Some(Value::Rel(sec, off))) if sec.obj == self.obj_index => Expr::Loc(sec, off),
            Ok(_) => e,
            Err(m) => return self.error(stmt, vcol, m),
        };
        let id = self.obj.symbol_id(name);
        let loc = self.loc_at(stmt, col);
        let sym = &mut self.obj.symbols[id as usize];
        match (&sym.def, kind) {
            (SymDef::Label { .. } | SymDef::Common { .. }, _) => {
                let prev = sym.def_loc;
                self.error(stmt, col, format!("symbol '{name}' is already defined"));
                self.diags.note(prev, "previous definition is here");
                return;
            }
            (SymDef::Equ(_), AssignKind::Equiv) if !sym.cmdline => {
                self.error(stmt, col, format!("symbol '{name}' is already defined"));
                return;
            }
            _ => {}
        }
        sym.def = SymDef::Equ(folded);
        sym.def_loc = Some(loc);
        sym.cmdline = false;
        sym.alias = alias;
    }

    fn dir_comm(
        &mut self,
        stmt: &Stmt,
        head: &Token,
        args: &[Token],
        lcomm: bool,
    ) -> Result<(), OpError> {
        let ops = split_commas(args);
        if ops.len() < 2 || ops.len() > 3 {
            return Err((head.col, "expected symbol, size[, align]".to_string()));
        }
        let [sym_tok] = ops[0] else {
            return Err((head.col, "expected a symbol name".to_string()));
        };
        let Some(name) = sym_tok.ident() else {
            return Err((sym_tok.col, "expected a symbol name".to_string()));
        };
        let size = self.abs_operand(stmt, ops[1])?;
        if !(0..=1 << 30).contains(&size) {
            return Err((ops[1][0].col, "invalid size".to_string()));
        }
        let size = size as u64;
        let align = match ops.get(2) {
            Some(op) => {
                let a = self.abs_operand(stmt, op)?;
                if a <= 0 || a > 1 << 20 || a & (a - 1) != 0 {
                    return Err((
                        op.first().map_or(head.col, |t| t.col),
                        "alignment must be a power of two".to_string(),
                    ));
                }
                a as u64
            }
            // Natural alignment of the size, at most 8 (GNU as default).
            None => match size {
                8.. => 8,
                4..=7 => 4,
                2..=3 => 2,
                _ => 1,
            },
        };
        let id = self.obj.symbol_id(name);
        let loc = self.loc_at(stmt, sym_tok.col);
        let sym = &self.obj.symbols[id as usize];
        match sym.def {
            SymDef::Undef => {}
            SymDef::Common {
                size: s0,
                align: a0,
            } if !lcomm && sym.bind != Bind::Local => {
                // Repeated .comm: keep the largest size and alignment.
                self.obj.symbols[id as usize].def = SymDef::Common {
                    size: s0.max(size),
                    align: a0.max(align),
                };
                return Ok(());
            }
            _ => return Err((sym_tok.col, format!("symbol '{name}' is already defined"))),
        }
        let local = lcomm || self.obj.symbols[id as usize].local_directive;
        if local {
            // Local commons are allocated in this file's .bss, like GNU as and LLVM.
            let bss = self.get_section(".bss", None, None, stmt.loc);
            let (saved_cur, saved_prev) = (self.cur, self.prev);
            self.cur = bss;
            self.align_to(align, Some(0), None, stmt, head.col);
            let off = self.cur_off();
            self.emit_fill(size, 0, stmt, head.col);
            self.cur = saved_cur;
            self.prev = saved_prev;
            let sym = &mut self.obj.symbols[id as usize];
            sym.def = SymDef::Label { sec: bss, off };
            sym.def_loc = Some(loc);
            if sym.kind == SymKind::NoType {
                sym.kind = SymKind::Object;
            }
            if sym.size.is_none() {
                sym.size = Some((Expr::Num(size as i64), loc));
            }
        } else {
            let sym = &mut self.obj.symbols[id as usize];
            sym.def = SymDef::Common { size, align };
            sym.def_loc = Some(loc);
            if sym.bind == Bind::Local {
                sym.bind = Bind::Global;
            }
            if sym.kind == SymKind::NoType {
                sym.kind = SymKind::Object;
            }
        }
        Ok(())
    }

    // ----- files -----------------------------------------------------------------------------

    fn dir_include(&mut self, head: &Token, args: &[Token]) -> Result<(), OpError> {
        let [Token {
            tok: Tok::Str(name),
            col,
            ..
        }] = args
        else {
            return Err((head.col, "expected a quoted file name".to_string()));
        };
        let name = String::from_utf8_lossy(name).into_owned();
        if self.file_depth() >= MAX_NESTING {
            return Err((*col, "includes nested too deeply".to_string()));
        }
        let Some(path) = self.resolve_path(&name) else {
            return Err((*col, format!("cannot find include file '{name}'")));
        };
        let text = std::fs::read(&path)
            .map_err(|e| (*col, format!("cannot read '{}': {e}", path.display())))?;
        let text = String::from_utf8_lossy(&text).into_owned();
        let shown = path.to_string_lossy().into_owned();
        let file = self.files.intern(&shown);
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.push_file(file, &text, dir);
        Ok(())
    }

    fn dir_incbin(&mut self, stmt: &Stmt, head: &Token, args: &[Token]) -> Result<(), OpError> {
        let ops = split_commas(args);
        let Some(
            [Token {
                tok: Tok::Str(name),
                col,
                ..
            }],
        ) = ops.first().copied()
        else {
            return Err((head.col, "expected a quoted file name".to_string()));
        };
        let col = *col;
        let name = String::from_utf8_lossy(name).into_owned();
        let Some(path) = self.resolve_path(&name) else {
            return Err((col, format!("cannot find file '{name}'")));
        };
        let data = std::fs::read(&path)
            .map_err(|e| (col, format!("cannot read '{}': {e}", path.display())))?;
        let skip = match ops.get(1) {
            Some(op) => self.abs_operand(stmt, op)?,
            None => 0,
        };
        if skip < 0 || skip as u64 > data.len() as u64 {
            return Err((col, "skip is outside the file".to_string()));
        }
        let rest = &data[skip as usize..];
        let count = match ops.get(2) {
            Some(op) => {
                let c = self.abs_operand(stmt, op)?;
                if c < 0 || c as u64 > rest.len() as u64 {
                    return Err((col, "count is outside the file".to_string()));
                }
                c as usize
            }
            None => rest.len(),
        };
        let bytes = rest[..count].to_vec();
        self.emit_bytes(&bytes, stmt, col);
        Ok(())
    }

    // ----- conditionals ------------------------------------------------------------------------

    fn dir_if(
        &mut self,
        name: &str,
        stmt: &Stmt,
        head: &Token,
        args: &[Token],
        raw: &str,
    ) -> Result<(), OpError> {
        let loc = stmt.loc;
        let value = match name {
            ".ifdef" | ".ifndef" | ".ifnotdef" => {
                let [t] = args else {
                    return Err((head.col, "expected a symbol name".to_string()));
                };
                let Some(sym) = t.ident() else {
                    return Err((t.col, "expected a symbol name".to_string()));
                };
                let defined = self
                    .obj
                    .find_symbol(sym)
                    .is_some_and(|id| self.obj.symbols[id as usize].is_defined());
                defined == (name == ".ifdef")
            }
            ".ifb" | ".ifnb" => raw.trim().is_empty() == (name == ".ifb"),
            ".ifc" | ".ifnc" => {
                let (a, b) =
                    split_ifc(raw).ok_or((head.col, "expected string1, string2".to_string()))?;
                (a == b) == (name == ".ifc")
            }
            _ => {
                let v = self.abs_operand(stmt, args)?;
                match name {
                    ".if" | ".ifne" => v != 0,
                    ".ifeq" => v == 0,
                    ".ifgt" => v > 0,
                    ".ifge" => v >= 0,
                    ".iflt" => v < 0,
                    _ => v <= 0,
                }
            }
        };
        self.cond.push(Cond {
            parent_active: true,
            active: value,
            taken: value,
            seen_else: false,
            loc,
        });
        Ok(())
    }

    fn dir_elseif(&mut self, stmt: &Stmt, head: &Token, args: &[Token]) -> Result<(), OpError> {
        let Some(top) = self.cond.last().copied() else {
            return Err((head.col, ".elseif without matching .if".to_string()));
        };
        if top.seen_else {
            return Err((head.col, ".elseif after .else".to_string()));
        }
        let mut next = top;
        if !top.parent_active || top.taken {
            next.active = false;
        } else {
            let v = self.abs_operand(stmt, args)?;
            next.active = v != 0;
            next.taken = v != 0;
        }
        if let Some(c) = self.cond.last_mut() {
            *c = next;
        }
        Ok(())
    }

    fn dir_else(&mut self, head: &Token) -> Result<(), OpError> {
        let Some(top) = self.cond.last_mut() else {
            return Err((head.col, ".else without matching .if".to_string()));
        };
        if top.seen_else {
            return Err((head.col, "duplicate .else".to_string()));
        }
        top.seen_else = true;
        top.active = top.parent_active && !top.taken;
        top.taken = true;
        Ok(())
    }

    // ----- macros and repetition ------------------------------------------------------------------

    /// Reads statements up to the matching `close` directive, tracking
    /// nesting of the `open` directives. Returns `None` at end of input.
    fn capture_block(&mut self, open: &[&str], close: &[&str]) -> Option<Vec<Stmt>> {
        let mut depth = 0usize;
        let mut body = Vec::new();
        while let Some(s) = self.next_stmt() {
            let w = first_word(&s.text);
            if open.contains(&w.as_str()) {
                depth += 1;
            } else if close.contains(&w.as_str()) {
                if depth == 0 {
                    return Some(body);
                }
                depth -= 1;
            }
            body.push(s);
        }
        None
    }

    fn dir_macro(&mut self, stmt: &Stmt, head: &Token, raw: &str) -> Result<(), OpError> {
        let raw = raw.trim();
        let name_len = raw.bytes().take_while(|&c| is_ident_char(c)).count();
        let name = &raw[..name_len];
        if name.is_empty() || !is_ident_start(name.as_bytes()[0]) {
            return Err((head.col, "expected a macro name".to_string()));
        }
        let mut params = Vec::new();
        for p in split_macro_args(raw[name_len..].trim_start_matches(',')) {
            let (lhs, default) = match p.split_once('=') {
                Some((l, d)) => (l.trim(), Some(d.trim().to_string())),
                None => (p.trim(), None),
            };
            let (pname, qual) = match lhs.split_once(':') {
                Some((n, q)) => (n.trim(), q.trim()),
                None => (lhs, ""),
            };
            if pname.is_empty() || !pname.bytes().all(is_ident_char) {
                return Err((head.col, format!("invalid macro parameter '{lhs}'")));
            }
            if !matches!(qual, "" | "req" | "vararg") {
                return Err((
                    head.col,
                    format!("unknown macro parameter qualifier '{qual}'"),
                ));
            }
            params.push(MacroParam {
                name: pname.to_string(),
                default,
                required: qual == "req",
                vararg: qual == "vararg",
            });
        }
        let name = name.to_string();
        let Some(body) = self.capture_block(&[".macro"], &[".endm", ".endmacro"]) else {
            return Err((head.col, format!("macro '{name}' is missing .endm")));
        };
        if self.macros.contains_key(&name) {
            self.warning(stmt, head.col, format!("macro '{name}' redefined"));
        }
        self.macros.insert(name, Rc::new(Macro { params, body }));
        Ok(())
    }

    /// Expands a macro invocation.
    pub(crate) fn expand_macro(&mut self, name: &str, m: Rc<Macro>, stmt: &Stmt, head: &Token) {
        if self.macro_depth() >= MAX_NESTING {
            self.error(
                stmt,
                head.col,
                format!("macro '{name}' expanded recursively too deeply"),
            );
            // Abandon the whole chain of expansions; continuing would only
            // repeat the error.
            if let Some(first) = self
                .stack
                .iter()
                .position(|f| matches!(f, Frame::Expansion(_)))
            {
                if let Frame::Expansion(e) = &self.stack[first] {
                    let depth = e.cond_depth;
                    self.cond.truncate(depth);
                }
                self.stack.truncate(first);
            }
            return;
        }
        let raw = raw_args(stmt, head);
        let args = split_macro_args(raw);
        let mut values: Vec<Option<String>> = vec![None; m.params.len()];
        let mut positional = 0;
        for (i, a) in args.iter().enumerate() {
            if let Some((k, v)) = a.split_once('=') {
                let k = k.trim();
                if let Some(p) = m.params.iter().position(|p| p.name == k) {
                    values[p] = Some(v.trim().to_string());
                    continue;
                }
            }
            if positional >= m.params.len() {
                return self.error(
                    stmt,
                    head.col,
                    format!("too many arguments for macro '{name}'"),
                );
            }
            if m.params[positional].vararg {
                values[positional] = Some(args[i..].join(", "));
                break;
            }
            values[positional] = Some(a.clone());
            positional += 1;
        }
        let mut subst = Vec::new();
        for (p, v) in m.params.iter().zip(values) {
            let v = match v {
                Some(v) if !v.is_empty() => v,
                _ => match &p.default {
                    Some(d) => d.clone(),
                    None if p.required => {
                        return self.error(
                            stmt,
                            head.col,
                            format!(
                                "missing value for required parameter '{}' of macro '{name}'",
                                p.name
                            ),
                        );
                    }
                    None => String::new(),
                },
            };
            subst.push((p.name.clone(), v));
        }
        let counter = self.expansion_counter;
        self.expansion_counter += 1;
        let stmts = m
            .body
            .iter()
            .map(|s| Stmt {
                text: substitute(&s.text, &subst, Some(counter)),
                loc: stmt.loc,
                exact_cols: false,
            })
            .collect();
        self.stack.push(Frame::Expansion(ExpFrame {
            stmts,
            idx: 0,
            macro_name: Some(name.to_string()),
            cond_depth: self.cond.len(),
        }));
    }

    fn dir_exitm(&mut self, head: &Token) -> Result<(), OpError> {
        let Some(pos) = self
            .stack
            .iter()
            .rposition(|f| matches!(f, Frame::Expansion(e) if e.macro_name.is_some()))
        else {
            return Err((head.col, ".exitm outside of a macro".to_string()));
        };
        if let Frame::Expansion(e) = &self.stack[pos] {
            let depth = e.cond_depth;
            self.cond.truncate(depth);
        }
        self.stack.truncate(pos);
        Ok(())
    }

    fn push_expansion(&mut self, stmts: Vec<Stmt>, head: &Token) -> Result<(), OpError> {
        if stmts.len() as u64 > self.budget {
            return Err((
                head.col,
                "repetition expands to too many statements".to_string(),
            ));
        }
        self.stack.push(Frame::Expansion(ExpFrame {
            stmts,
            idx: 0,
            macro_name: None,
            cond_depth: self.cond.len(),
        }));
        Ok(())
    }

    fn dir_rept(&mut self, stmt: &Stmt, head: &Token, args: &[Token]) -> Result<(), OpError> {
        let count = self.abs_operand(stmt, args);
        let Some(body) = self.capture_block(&[".rept", ".irp", ".irpc"], &[".endr"]) else {
            return Err((head.col, ".rept is missing .endr".to_string()));
        };
        let count = count?;
        if count < 0 {
            return Err((head.col, "repeat count must not be negative".to_string()));
        }
        if body.is_empty() {
            return Ok(());
        }
        if (count as u64).saturating_mul(body.len() as u64) > self.budget {
            return Err((
                head.col,
                "repetition expands to too many statements".to_string(),
            ));
        }
        let mut stmts = Vec::with_capacity(count as usize * body.len());
        for _ in 0..count {
            stmts.extend(body.iter().cloned());
        }
        self.push_expansion(stmts, head)
    }

    fn dir_irp(
        &mut self,
        _stmt: &Stmt,
        head: &Token,
        raw: &str,
        chars: bool,
    ) -> Result<(), OpError> {
        let raw = raw.trim();
        let name_len = raw.bytes().take_while(|&c| is_ident_char(c)).count();
        let name = raw[..name_len].to_string();
        let rest = raw[name_len..].trim_start();
        let rest = rest.strip_prefix(',').unwrap_or(rest).trim();
        let Some(body) = self.capture_block(&[".rept", ".irp", ".irpc"], &[".endr"]) else {
            return Err((head.col, ".irp is missing .endr".to_string()));
        };
        if name.is_empty() {
            return Err((head.col, "expected a parameter name".to_string()));
        }
        let values: Vec<String> = if chars {
            unquote(rest).chars().map(|c| c.to_string()).collect()
        } else {
            let v = split_macro_args(rest);
            if v.is_empty() {
                vec![String::new()]
            } else {
                v
            }
        };
        let mut stmts = Vec::new();
        for v in values {
            let subst = [(name.clone(), v)];
            for s in &body {
                stmts.push(Stmt {
                    text: substitute(&s.text, &subst, None),
                    ..s.clone()
                });
                if stmts.len() as u64 > self.budget {
                    return Err((
                        head.col,
                        "repetition expands to too many statements".to_string(),
                    ));
                }
            }
        }
        self.push_expansion(stmts, head)
    }

    // ----- debug information ---------------------------------------------------------------------

    fn dir_file(&mut self, args: &[Token]) -> Result<(), OpError> {
        // `.file "name"` (no number) only names the translation unit: ignored.
        let Some(Token {
            tok: Tok::Int(n),
            col,
            ..
        }) = args.first()
        else {
            return Ok(());
        };
        let strings: Vec<String> = args[1..]
            .iter()
            .map_while(|t| match &t.tok {
                Tok::Str(s) => Some(String::from_utf8_lossy(s).into_owned()),
                _ => None,
            })
            .collect();
        let (dir, name) = match strings.as_slice() {
            [name] => (None, name.clone()),
            [dir, name, ..] => (Some(dir.clone()), name.clone()),
            [] => return Err((*col, "expected a file name".to_string())),
        };
        if *n < 0 {
            return Err((*col, "file number must not be negative".to_string()));
        }
        if *n == 0 {
            self.comp_dir = dir.clone();
        }
        let path = dwarf_path(dir.as_deref(), &name, self.comp_dir.as_deref());
        let id = self.files.intern(&path);
        self.dwarf_files.insert(*n as u64, id);
        Ok(())
    }

    fn dir_loc(&mut self, args: &[Token], head: &Token) -> Result<(), OpError> {
        let nums: Vec<i64> = args
            .iter()
            .map_while(|t| match t.tok {
                Tok::Int(v) => Some(v),
                _ => None,
            })
            .collect();
        if nums.len() < 2 {
            return Err((head.col, "expected file number and line".to_string()));
        }
        let Some(&file) = self.dwarf_files.get(&(nums[0] as u64)) else {
            return Err((
                args[0].col,
                format!("file number {} was not declared with .file", nums[0]),
            ));
        };
        let mut is_stmt = self.is_stmt;
        let rest = &args[nums.len()..];
        let mut i = 0;
        while i < rest.len() {
            if rest[i].ident() == Some("is_stmt") {
                match rest.get(i + 1).map(|t| &t.tok) {
                    Some(Tok::Int(v)) => is_stmt = *v != 0,
                    _ => return Err((rest[i].col, "is_stmt requires a value".to_string())),
                }
                i += 1;
            }
            i += 1;
        }
        self.is_stmt = is_stmt;
        self.pending_loc = Some(PendingLoc {
            file,
            line: nums[1].clamp(0, i64::from(u32::MAX)) as u32,
            col: nums
                .get(2)
                .copied()
                .unwrap_or(0)
                .clamp(0, i64::from(u32::MAX)) as u32,
            is_stmt,
        });
        Ok(())
    }

    fn dir_option(&mut self, stmt: &Stmt, head: &Token, args: &[Token]) -> Result<(), OpError> {
        match args.first().and_then(|t| t.ident()) {
            Some("push") => self.option_stack.push(stmt.loc),
            Some("pop") => {
                if self.option_stack.pop().is_none() {
                    return Err((head.col, ".option pop without .option push".to_string()));
                }
            }
            Some(_) => {}
            None => return Err((head.col, "expected an option".to_string())),
        }
        Ok(())
    }
}

/// Raw text of a statement after its directive/mnemonic token.
fn raw_args<'s>(stmt: &'s Stmt, head: &Token) -> &'s str {
    stmt.text
        .get((head.col + head.len) as usize..)
        .unwrap_or("")
}

/// First whitespace-delimited word, lowercased.
pub(crate) fn first_word(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Splits raw text at top-level commas (outside quotes and parentheses),
/// returning each piece with its byte offset.
fn split_raw(s: &str) -> Vec<(usize, &str)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut start, mut i) = (0i32, 0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth == 0 => {
                out.push((start, &s[start..i]));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if !s.trim().is_empty() || !out.is_empty() {
        out.push((start, &s[start.min(s.len())..]));
    }
    out
}

/// Splits macro arguments: at top-level commas if there are any, otherwise
/// at whitespace. Quotes and parentheses group.
pub(crate) fn split_macro_args(s: &str) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return Vec::new();
    }
    let pieces = split_raw(s);
    if pieces.len() > 1 {
        return pieces
            .into_iter()
            .map(|(_, p)| p.trim().to_string())
            .collect();
    }
    // Whitespace separated; keep quoted strings and parenthesized groups intact.
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                cur.push_str(&s[start..(i + 1).min(s.len())]);
            }
            b'(' => {
                depth += 1;
                cur.push('(');
            }
            b')' => {
                depth -= 1;
                cur.push(')');
            }
            c if c.is_ascii_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => {
                let ch_len = s[i..].chars().next().map_or(1, char::len_utf8);
                cur.push_str(&s[i..i + ch_len]);
                i += ch_len - 1;
            }
        }
        i += 1;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Replaces `\param`, `\@` and `\()` in a macro or `.irp` body line.
pub(crate) fn substitute(text: &str, params: &[(String, String)], counter: Option<u64>) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            let next = b[i + 1];
            if next == b'@' {
                if let Some(c) = counter {
                    out.push_str(&c.to_string());
                    i += 2;
                    continue;
                }
            } else if next == b'(' && b.get(i + 2) == Some(&b')') {
                i += 3;
                continue;
            } else if is_ident_char(next) {
                let end = i + 1 + b[i + 1..].iter().take_while(|&&c| is_ident_char(c)).count();
                let ident = &text[i + 1..end];
                if let Some((_, v)) = params.iter().find(|(n, _)| n == ident) {
                    out.push_str(v);
                    i = end;
                    continue;
                }
            }
        }
        let ch_len = text[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Removes surrounding double quotes, if present.
fn unquote(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(s)
        .to_string()
}

fn split_ifc(raw: &str) -> Option<(String, String)> {
    let parts = split_raw(raw);
    match parts.as_slice() {
        [(_, a), (_, b)] => Some((unquote(a), unquote(b))),
        _ => None,
    }
}

/// The text of a single string operand.
fn string_arg(args: &[Token]) -> Option<String> {
    match args {
        [Token {
            tok: Tok::Str(s), ..
        }] => Some(String::from_utf8_lossy(s).into_owned()),
        _ => None,
    }
}

/// Checks that `v` fits in `size` bytes (signed or unsigned).
fn check_data_range(v: i64, size: u8) -> Result<(), String> {
    if size >= 8 {
        return Ok(());
    }
    let bits = u32::from(size) * 8;
    let min = -(1i64 << (bits - 1));
    let max = (1i64 << bits) - 1;
    if (min..=max).contains(&v) {
        Ok(())
    } else {
        Err(format!("value {v} does not fit in {size} byte(s)"))
    }
}

/// Path of a DWARF `.file` entry for the debug info.
///
/// An absolute `name` is used as is. A relative `name` is joined with a
/// relative directory; with an absolute directory it is kept relative when
/// that directory is the compilation directory (file 0's), so paths stay
/// relative to where the compiler ran, and joined otherwise.
pub(crate) fn dwarf_path(dir: Option<&str>, name: &str, comp_dir: Option<&str>) -> String {
    let join = |d: &str| {
        let d = d.trim_end_matches('/');
        if d.is_empty() || d == "." {
            name.to_string()
        } else {
            format!("{d}/{name}")
        }
    };
    match dir {
        _ if name.starts_with('/') => name.to_string(),
        None => name.to_string(),
        Some(d) if !d.starts_with('/') => join(d),
        Some(d) if comp_dir == Some(d) => name.to_string(),
        Some(d) => join(d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_arg_splitting() {
        assert_eq!(
            split_macro_args("a, b c, (d, e)"),
            vec!["a", "b c", "(d, e)"]
        );
        assert_eq!(split_macro_args("a b  c"), vec!["a", "b", "c"]);
        assert_eq!(split_macro_args("\"x y\" 4(sp)"), vec!["\"x y\"", "4(sp)"]);
        assert_eq!(split_macro_args("  "), Vec::<String>::new());
        assert_eq!(split_macro_args("a,"), vec!["a", ""]);
    }

    #[test]
    fn substitution() {
        let p = [
            ("reg".to_string(), "a0".to_string()),
            ("n".to_string(), "4".to_string()),
        ];
        assert_eq!(
            substitute("addi \\reg, \\reg, \\n", &p, None),
            "addi a0, a0, 4"
        );
        assert_eq!(
            substitute("lbl\\@: \\reg\\()x \\unknown", &p, Some(7)),
            "lbl7: a0x \\unknown"
        );
        assert_eq!(substitute("\\@", &p, None), "\\@");
    }

    #[test]
    fn dwarf_paths() {
        let comp = Some("/home/u/strata");
        assert_eq!(
            dwarf_path(Some("/home/u/strata"), "sw/kernel/main.c", comp),
            "sw/kernel/main.c"
        );
        assert_eq!(
            dwarf_path(Some("sw/include"), "machine.h", comp),
            "sw/include/machine.h"
        );
        assert_eq!(
            dwarf_path(Some("/usr/include"), "stdint.h", comp),
            "/usr/include/stdint.h"
        );
        assert_eq!(dwarf_path(Some("/x"), "/abs/f.c", comp), "/abs/f.c");
        assert_eq!(dwarf_path(None, "f.c", comp), "f.c");
        assert_eq!(dwarf_path(Some("."), "f.c", comp), "f.c");
    }

    #[test]
    fn raw_split() {
        assert_eq!(
            split_raw(".text.x,\"ax\",@progbits"),
            vec![(0, ".text.x"), (8, "\"ax\""), (13, "@progbits")]
        );
        assert_eq!(split_raw("\"a,b\", c"), vec![(0, "\"a,b\""), (6, " c")]);
        assert_eq!(split_raw(""), Vec::<(usize, &str)>::new());
    }

    #[test]
    fn data_ranges() {
        assert!(check_data_range(255, 1).is_ok());
        assert!(check_data_range(-128, 1).is_ok());
        assert!(check_data_range(256, 1).is_err());
        assert!(check_data_range(-129, 1).is_err());
        assert!(check_data_range(0xFFFF_FFFF, 4).is_ok());
        assert!(check_data_range(0x1_0000_0000, 4).is_err());
        assert!(check_data_range(i64::MIN, 8).is_ok());
    }
}
