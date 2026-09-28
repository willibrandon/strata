//! Instructions and pseudo-instructions.

use super::{split_commas, Assembler, OpError, Stmt};
use crate::expr::{Expr, Value};
use crate::lexer::{Punct, Tok, Token};
use crate::object::FixKind;
use strata_isa::encode::{
    b_type, fits_signed, hi20, i_type, j_type, lo12, op_fields, r_type, s_type, u_type,
};
use strata_isa::{csr_by_name, reg_by_name, Op};

/// Relocation operator applied to an immediate operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RelocOp {
    Hi,
    Lo,
    PcrelHi,
    PcrelLo,
}

/// An immediate operand as written.
#[derive(Clone, Debug)]
enum Imm {
    Plain(Expr, u32),
    Reloc(RelocOp, Expr, u32),
}

/// An immediate either known now or to be patched later.
enum Resolved {
    Val(i32),
    Fix(FixKind, Expr),
}

const RA: u8 = 1;
const T1: u8 = 6;

impl Assembler<'_> {
    /// Assembles instruction `mn` (lowercase) with operand tokens `args`.
    pub(crate) fn instruction(&mut self, mn: &str, stmt: &Stmt, head: &Token, args: &[Token]) {
        let ops = split_commas(args);
        if let Some(empty) = ops.iter().position(|o| o.is_empty()) {
            let col = if empty == 0 {
                head.col + head.len
            } else {
                ops[empty - 1].last().map_or(head.col, |t| t.col + t.len)
            };
            return self.error(stmt, col, "expected an operand");
        }
        let ctx = Ctx {
            stmt,
            head,
            ops: &ops,
        };
        if let Err((col, msg)) = self.insn_inner(mn, &ctx) {
            self.error(stmt, col, msg);
        }
    }

    fn insn_inner(&mut self, mn: &str, c: &Ctx) -> Result<(), OpError> {
        use Op::*;
        let base = Op::from_mnemonic(mn);
        match mn {
            "add" | "sub" | "sll" | "slt" | "sltu" | "xor" | "srl" | "sra" | "or" | "and" => {
                let op = base.unwrap_or(Illegal);
                c.count(3)?;
                let (rd, rs1) = (reg(c.ops[0])?, reg(c.ops[1])?);
                // `add rd, rs, imm` and friends mean the immediate form (GNU as, LLVM).
                let imm_op = match op {
                    Add => Some(Addi),
                    And => Some(Andi),
                    Or => Some(Ori),
                    Xor => Some(Xori),
                    Slt => Some(Slti),
                    Sltu => Some(Sltiu),
                    Sll => Some(Slli),
                    Srl => Some(Srli),
                    Sra => Some(Srai),
                    _ => None,
                };
                match (imm_op, is_reg(c.ops[2])) {
                    (Some(iop), false) if iop.format() == strata_isa::Format::Shift => {
                        self.shift(iop, rd, rs1, c.ops[2], c.stmt)?
                    }
                    (Some(iop), false) => {
                        let imm = self.imm(c.stmt, c.ops[2])?;
                        self.i_with(iop, rd, rs1, imm, c.stmt)?;
                    }
                    _ => {
                        let rs2 = reg(c.ops[2])?;
                        self.r(op, rd, rs1, rs2, c.stmt);
                    }
                }
            }
            "addi" | "slti" | "sltiu" | "xori" | "ori" | "andi" => {
                let op = base.unwrap_or(Illegal);
                c.count(3)?;
                let (rd, rs1) = (reg(c.ops[0])?, reg(c.ops[1])?);
                let imm = self.imm(c.stmt, c.ops[2])?;
                self.i_with(op, rd, rs1, imm, c.stmt)?;
            }
            "slli" | "srli" | "srai" => {
                let op = base.unwrap_or(Illegal);
                c.count(3)?;
                let (rd, rs1) = (reg(c.ops[0])?, reg(c.ops[1])?);
                self.shift(op, rd, rs1, c.ops[2], c.stmt)?;
            }
            "lb" | "lh" | "lw" | "lbu" | "lhu" => {
                let op = base.unwrap_or(Illegal);
                c.count(2)?;
                let rd = reg(c.ops[0])?;
                if is_mem(c.ops[1]) || looks_like_mem(c.ops[1]) {
                    let (imm, rs1) = self.mem(c.stmt, c.ops[1])?;
                    self.i_with(op, rd, rs1, imm, c.stmt)?;
                } else {
                    let target = self.symbol_operand(c.stmt, c.ops[1])?;
                    self.pcrel_pair(
                        rd,
                        target,
                        |hi_off| PairSecond::I(op, rd, rd, hi_off),
                        c.stmt,
                    );
                }
            }
            "sb" | "sh" | "sw" => {
                let op = base.unwrap_or(Illegal);
                let rs2 = reg(c.ops.first().copied().unwrap_or(&[]))?;
                match c.ops.len() {
                    2 => {
                        let (imm, rs1) = self.mem(c.stmt, c.ops[1])?;
                        self.s_with(op, rs1, rs2, imm, c.stmt)?;
                    }
                    3 => {
                        let target = self.symbol_operand(c.stmt, c.ops[1])?;
                        let rt = reg(c.ops[2])?;
                        self.pcrel_pair(
                            rt,
                            target,
                            |hi_off| PairSecond::S(op, rt, rs2, hi_off),
                            c.stmt,
                        );
                    }
                    _ => c.count(2)?,
                }
            }
            "beq" | "bne" | "blt" | "bge" | "bltu" | "bgeu" => {
                c.count(3)?;
                let (rs1, rs2) = (reg(c.ops[0])?, reg(c.ops[1])?);
                self.branch(base.unwrap_or(Illegal), rs1, rs2, c.ops[2], c.stmt)?;
            }
            "bgt" | "ble" | "bgtu" | "bleu" => {
                c.count(3)?;
                let op = match mn {
                    "bgt" => Blt,
                    "ble" => Bge,
                    "bgtu" => Bltu,
                    _ => Bgeu,
                };
                let (rs, rt) = (reg(c.ops[0])?, reg(c.ops[1])?);
                self.branch(op, rt, rs, c.ops[2], c.stmt)?;
            }
            "beqz" | "bnez" | "bltz" | "bgez" | "blez" | "bgtz" => {
                c.count(2)?;
                let rs = reg(c.ops[0])?;
                let (op, rs1, rs2) = match mn {
                    "beqz" => (Beq, rs, 0),
                    "bnez" => (Bne, rs, 0),
                    "bltz" => (Blt, rs, 0),
                    "bgez" => (Bge, rs, 0),
                    "blez" => (Bge, 0, rs),
                    _ => (Blt, 0, rs),
                };
                self.branch(op, rs1, rs2, c.ops[1], c.stmt)?;
            }
            "lui" | "auipc" => {
                let op = base.unwrap_or(Illegal);
                c.count(2)?;
                let rd = reg(c.ops[0])?;
                let imm = self.imm(c.stmt, c.ops[1])?;
                self.upper(op, rd, imm, c.stmt)?;
            }
            "jal" => match c.ops.len() {
                1 => self.jal(RA, c.ops[0], c.stmt)?,
                2 => {
                    let rd = reg(c.ops[0])?;
                    self.jal(rd, c.ops[1], c.stmt)?
                }
                _ => c.count(2)?,
            },
            "j" => {
                c.count(1)?;
                self.jal(0, c.ops[0], c.stmt)?;
            }
            "jalr" => match c.ops.len() {
                1 => self.jalr_mem_or_reg(RA, c.ops[0], c.stmt)?,
                2 => {
                    let rd = reg(c.ops[0])?;
                    self.jalr_mem_or_reg(rd, c.ops[1], c.stmt)?
                }
                3 => {
                    let (rd, rs1) = (reg(c.ops[0])?, reg(c.ops[1])?);
                    let imm = self.imm(c.stmt, c.ops[2])?;
                    self.i_with(Jalr, rd, rs1, imm, c.stmt)?;
                }
                _ => c.count(3)?,
            },
            "jr" => match c.ops.len() {
                1 => self.jalr_mem_or_reg(0, c.ops[0], c.stmt)?,
                2 => {
                    let rs1 = reg(c.ops[0])?;
                    let imm = self.imm(c.stmt, c.ops[1])?;
                    self.i_with(Jalr, 0, rs1, imm, c.stmt)?;
                }
                _ => c.count(1)?,
            },
            "ret" => {
                c.count(0)?;
                self.i_val(Jalr, 0, RA, 0, c.stmt);
            }
            "call" | "tail" => {
                let (rd, target_op) = match (mn, c.ops.len()) {
                    ("call", 1) => (RA, c.ops[0]),
                    ("call", 2) => (reg(c.ops[0])?, c.ops[1]),
                    ("tail", 1) => (T1, c.ops[0]),
                    _ => return c.count(1),
                };
                let target = self.call_target(c.stmt, target_op)?;
                let link = if mn == "call" { rd } else { 0 };
                self.pcrel_pair(
                    rd,
                    target,
                    |hi_off| PairSecond::I(Jalr, link, rd, hi_off),
                    c.stmt,
                );
            }
            "jump" => {
                c.count(2)?;
                let target = self.call_target(c.stmt, c.ops[0])?;
                let rt = reg(c.ops[1])?;
                self.pcrel_pair(
                    rt,
                    target,
                    |hi_off| PairSecond::I(Jalr, 0, rt, hi_off),
                    c.stmt,
                );
            }
            "la" | "lla" => {
                c.count(2)?;
                let rd = reg(c.ops[0])?;
                not_reg(c.ops[1])?;
                let col = c.ops[1][0].col;
                let target = self.parse_expr(c.stmt, c.ops[1])?;
                // A constant address is simply loaded (as GNU as and LLVM do).
                match self.eval_now(&target).map_err(|m| (col, m))? {
                    Some(Value::Abs(v)) => self.li(rd, v, col, c.stmt)?,
                    _ => self.pcrel_pair(
                        rd,
                        target,
                        |hi_off| PairSecond::I(Addi, rd, rd, hi_off),
                        c.stmt,
                    ),
                }
            }
            "li" => {
                c.count(2)?;
                let rd = reg(c.ops[0])?;
                not_reg(c.ops[1])?;
                let col = c.ops[1][0].col;
                let v = self.abs_operand(c.stmt, c.ops[1])?;
                self.li(rd, v, col, c.stmt)?;
            }
            "nop" => {
                c.count(0)?;
                self.i_val(Addi, 0, 0, 0, c.stmt);
            }
            "mv" | "not" | "neg" | "seqz" | "snez" | "sltz" | "sgtz" | "zext.b" => {
                c.count(2)?;
                let (rd, rs) = (reg(c.ops[0])?, reg(c.ops[1])?);
                match mn {
                    "mv" => self.i_val(Addi, rd, rs, 0, c.stmt),
                    "not" => self.i_val(Xori, rd, rs, -1, c.stmt),
                    "neg" => self.r(Sub, rd, 0, rs, c.stmt),
                    "seqz" => self.i_val(Sltiu, rd, rs, 1, c.stmt),
                    "snez" => self.r(Sltu, rd, 0, rs, c.stmt),
                    "sltz" => self.r(Slt, rd, rs, 0, c.stmt),
                    "sgtz" => self.r(Slt, rd, 0, rs, c.stmt),
                    _ => self.i_val(Andi, rd, rs, 255, c.stmt),
                }
            }
            "csrrw" | "csrrs" | "csrrc" => {
                c.count(3)?;
                let rd = reg(c.ops[0])?;
                let csr = self.csr(c.stmt, c.ops[1])?;
                let op = base.unwrap_or(Illegal);
                if is_reg(c.ops[2]) {
                    let rs1 = reg(c.ops[2])?;
                    self.csr_insn(op, rd, csr, rs1, c.stmt);
                } else {
                    // An immediate source selects the `...i` form (GNU as, LLVM).
                    let uimm = self.uimm5(c.stmt, c.ops[2])?;
                    self.csr_insn(csr_imm_form(op), rd, csr, uimm, c.stmt);
                }
            }
            "csrrwi" | "csrrsi" | "csrrci" => {
                c.count(3)?;
                let rd = reg(c.ops[0])?;
                let csr = self.csr(c.stmt, c.ops[1])?;
                let uimm = self.uimm5(c.stmt, c.ops[2])?;
                self.csr_insn(base.unwrap_or(Illegal), rd, csr, uimm, c.stmt);
            }
            "csrr" => {
                c.count(2)?;
                let rd = reg(c.ops[0])?;
                let csr = self.csr(c.stmt, c.ops[1])?;
                self.csr_insn(Csrrs, rd, csr, 0, c.stmt);
            }
            "csrw" | "csrs" | "csrc" => {
                c.count(2)?;
                let csr = self.csr(c.stmt, c.ops[0])?;
                let op = match mn {
                    "csrw" => Csrrw,
                    "csrs" => Csrrs,
                    _ => Csrrc,
                };
                if is_reg(c.ops[1]) {
                    let rs = reg(c.ops[1])?;
                    self.csr_insn(op, 0, csr, rs, c.stmt);
                } else {
                    let uimm = self.uimm5(c.stmt, c.ops[1])?;
                    self.csr_insn(csr_imm_form(op), 0, csr, uimm, c.stmt);
                }
            }
            "csrwi" | "csrsi" | "csrci" => {
                c.count(2)?;
                let csr = self.csr(c.stmt, c.ops[0])?;
                let uimm = self.uimm5(c.stmt, c.ops[1])?;
                let op = match mn {
                    "csrwi" => Csrrwi,
                    "csrsi" => Csrrsi,
                    _ => Csrrci,
                };
                self.csr_insn(op, 0, csr, uimm, c.stmt);
            }
            "rdcycle" | "rdcycleh" | "rdtime" | "rdtimeh" | "rdinstret" | "rdinstreth" => {
                c.count(1)?;
                let rd = reg(c.ops[0])?;
                let csr = csr_by_name(&mn[2..]).unwrap_or(0xC00);
                self.csr_insn(Csrrs, rd, csr, 0, c.stmt);
            }
            "unimp" => {
                c.count(0)?;
                self.csr_insn(Csrrw, 0, 0xC00, 0, c.stmt);
            }
            "fence" => {
                let (pred, succ) = match c.ops.len() {
                    0 => (0xF, 0xF),
                    2 => (fence_set(c.ops[0])?, fence_set(c.ops[1])?),
                    _ => return c.count(2),
                };
                self.fence(0, pred, succ, c.stmt);
            }
            "fence.tso" => {
                c.count(0)?;
                self.fence(8, 3, 3, c.stmt);
            }
            "pause" => {
                c.count(0)?;
                self.fence(0, 1, 0, c.stmt);
            }
            "fence.i" => {
                c.count(0)?;
                self.emit_insn(i_type(op_fields(FenceI).0, 0, 1, 0, 0), None, c.stmt);
            }
            "ecall" | "ebreak" | "mret" | "wfi" => {
                c.count(0)?;
                let op = base.unwrap_or(Illegal);
                let word = strata_isa::encode(&strata_isa::Insn::new(op)).unwrap_or(0);
                self.emit_insn(word, None, c.stmt);
            }
            _ => return Err((c.head.col, format!("unknown instruction '{mn}'"))),
        }
        Ok(())
    }

    // ----- operands ------------------------------------------------------------------------

    /// Parses an immediate operand, recognizing `%hi(...)` and friends.
    fn imm(&mut self, stmt: &Stmt, op: &[Token]) -> Result<Imm, OpError> {
        not_reg(op)?;
        let col = op[0].col;
        if op[0].is_punct(Punct::Percent) {
            let Some(name) = op.get(1).and_then(|t| t.ident()) else {
                return Err((col, "expected a relocation operator after '%'".into()));
            };
            let rel = match name.to_ascii_lowercase().as_str() {
                "hi" => RelocOp::Hi,
                "lo" => RelocOp::Lo,
                "pcrel_hi" => RelocOp::PcrelHi,
                "pcrel_lo" => RelocOp::PcrelLo,
                other => {
                    return Err((
                        op[1].col,
                        format!("unsupported relocation operator '%{other}'"),
                    ))
                }
            };
            let n = op.len();
            if n < 4
                || !op[2].is_punct(Punct::LParen)
                || !op[n - 1].is_punct(Punct::RParen)
                || !balanced(&op[3..n - 1])
            {
                return Err((col, format!("expected '%{name}(expression)'")));
            }
            let e = self.parse_expr(stmt, &op[3..n - 1])?;
            return Ok(Imm::Reloc(rel, e, col));
        }
        Ok(Imm::Plain(self.parse_expr(stmt, op)?, col))
    }

    /// Parses `offset(reg)`, `(reg)` or `%lo(sym)(reg)`.
    fn mem(&mut self, stmt: &Stmt, op: &[Token]) -> Result<(Imm, u8), OpError> {
        if looks_like_mem(op) && !is_mem(op) {
            let t = &op[op.len() - 2];
            return Err((t.col, format!("expected a register, found {}", t.tok)));
        }
        if !is_mem(op) {
            return Err((
                op[0].col,
                "expected a memory operand 'offset(register)'".into(),
            ));
        }
        let n = op.len();
        let base = reg(&op[n - 2..n - 1])?;
        let imm = if n == 3 {
            Imm::Plain(Expr::Num(0), op[0].col)
        } else {
            self.imm(stmt, &op[..n - 3])?
        };
        Ok((imm, base))
    }

    /// A symbolic target of `la`, `lw rd, sym` and similar: not a plain number.
    fn symbol_operand(&mut self, stmt: &Stmt, op: &[Token]) -> Result<Expr, OpError> {
        not_reg(op)?;
        let e = self.parse_expr(stmt, op)?;
        if e.is_constant() {
            return Err((op[0].col, "operand must be a symbol".into()));
        }
        Ok(e)
    }

    /// Target of `call`/`tail`/`jump`; an `@plt` suffix is accepted and ignored.
    fn call_target(&mut self, stmt: &Stmt, op: &[Token]) -> Result<Expr, OpError> {
        let n = op.len();
        let op = if n >= 3
            && op[n - 2].is_punct(Punct::At)
            && op[n - 1]
                .ident()
                .is_some_and(|s| s.eq_ignore_ascii_case("plt"))
        {
            &op[..n - 2]
        } else {
            op
        };
        self.symbol_operand(stmt, op)
    }

    fn csr(&mut self, stmt: &Stmt, op: &[Token]) -> Result<u16, OpError> {
        if let [t] = op {
            if let Some(n) = t.ident().and_then(csr_by_name) {
                return Ok(n);
            }
        }
        let col = op[0].col;
        let e = self.parse_expr(stmt, op)?;
        match self.eval_now(&e).map_err(|m| (col, m))? {
            Some(Value::Abs(v)) if (0..4096).contains(&v) => Ok(v as u16),
            _ => Err((
                col,
                "operand must be a valid CSR name or an integer in the range [0, 4095]".into(),
            )),
        }
    }

    fn uimm5(&mut self, stmt: &Stmt, op: &[Token]) -> Result<u8, OpError> {
        not_reg(op)?;
        let col = op[0].col;
        let v = self.abs_operand(stmt, op)?;
        if !(0..32).contains(&v) {
            return Err((
                col,
                "immediate must be an integer in the range [0, 31]".into(),
            ));
        }
        Ok(v as u8)
    }

    // ----- encoders ---------------------------------------------------------------------------

    /// `li`: `addi` if the value fits in 12 bits, else `lui` + optional `addi`.
    fn li(&mut self, rd: u8, v: i64, col: u32, stmt: &Stmt) -> Result<(), OpError> {
        if !(-(1i64 << 31)..(1i64 << 32)).contains(&v) {
            return Err((col, "immediate must fit in 32 bits".into()));
        }
        let v = i64::from(v as u32 as i32);
        if fits_signed(v, 12) {
            self.i_val(Op::Addi, rd, 0, v as i32, stmt);
        } else {
            self.emit_insn(
                u_type(op_fields(Op::Lui).0, rd, (hi20(v) << 12) as i32),
                None,
                stmt,
            );
            if lo12(v) != 0 {
                self.i_val(Op::Addi, rd, rd, lo12(v), stmt);
            }
        }
        Ok(())
    }

    /// Shift by an immediate amount (0..=31).
    fn shift(
        &mut self,
        op: Op,
        rd: u8,
        rs1: u8,
        amount: &[Token],
        stmt: &Stmt,
    ) -> Result<(), OpError> {
        not_reg(amount)?;
        let col = amount[0].col;
        let e = self.parse_expr(stmt, amount)?;
        let (opc, f3, f7) = op_fields(op);
        let word = |sh: i32| i_type(opc, rd, f3, rs1, sh | (f7 << 5) as i32);
        match self.eval_now(&e).map_err(|m| (col, m))? {
            Some(Value::Abs(v)) if (0..32).contains(&v) => {
                self.emit_insn(word(v as i32), None, stmt)
            }
            Some(_) => {
                return Err((
                    col,
                    "shift amount must be an integer in the range [0, 31]".into(),
                ))
            }
            None => self.emit_insn(word(0), Some((FixKind::Shamt, e)), stmt),
        }
        Ok(())
    }

    fn r(&mut self, op: Op, rd: u8, rs1: u8, rs2: u8, stmt: &Stmt) {
        let (opc, f3, f7) = op_fields(op);
        self.emit_insn(r_type(opc, rd, f3, rs1, rs2, f7), None, stmt);
    }

    fn i_val(&mut self, op: Op, rd: u8, rs1: u8, imm: i32, stmt: &Stmt) {
        let (opc, f3, _) = op_fields(op);
        self.emit_insn(i_type(opc, rd, f3, rs1, imm), None, stmt);
    }

    fn csr_insn(&mut self, op: Op, rd: u8, csr: u16, rs1_or_uimm: u8, stmt: &Stmt) {
        let (opc, f3, _) = op_fields(op);
        self.emit_insn(i_type(opc, rd, f3, rs1_or_uimm, i32::from(csr)), None, stmt);
    }

    fn fence(&mut self, fm: u32, pred: u32, succ: u32, stmt: &Stmt) {
        let imm = (fm << 8 | pred << 4 | succ) as i32;
        self.emit_insn(i_type(op_fields(Op::Fence).0, 0, 0, 0, imm), None, stmt);
    }

    /// Resolves a 12-bit I/S immediate.
    fn resolve12(&mut self, imm: Imm, store: bool) -> Result<Resolved, OpError> {
        let range_msg = "operand must be a symbol with a %lo/%pcrel_lo modifier or an integer in the range [-2048, 2047]";
        Ok(match imm {
            Imm::Plain(e, col) => match self.eval_now(&e).map_err(|m| (col, m))? {
                Some(Value::Abs(v)) if fits_signed(v, 12) => Resolved::Val(v as i32),
                Some(_) => return Err((col, range_msg.into())),
                None => Resolved::Fix(if store { FixKind::ImmS } else { FixKind::ImmI }, e),
            },
            Imm::Reloc(RelocOp::Lo, e, col) => match self.eval_now(&e).map_err(|m| (col, m))? {
                Some(Value::Abs(v)) => Resolved::Val(lo12(v)),
                _ => Resolved::Fix(
                    if store {
                        FixKind::Lo12S
                    } else {
                        FixKind::Lo12I
                    },
                    e,
                ),
            },
            Imm::Reloc(RelocOp::PcrelLo, e, _) => Resolved::Fix(
                if store {
                    FixKind::PcrelLo12S(None)
                } else {
                    FixKind::PcrelLo12I(None)
                },
                e,
            ),
            Imm::Reloc(_, _, col) => return Err((col, range_msg.into())),
        })
    }

    fn i_with(&mut self, op: Op, rd: u8, rs1: u8, imm: Imm, stmt: &Stmt) -> Result<(), OpError> {
        let (opc, f3, _) = op_fields(op);
        match self.resolve12(imm, false)? {
            Resolved::Val(v) => self.emit_insn(i_type(opc, rd, f3, rs1, v), None, stmt),
            Resolved::Fix(k, e) => self.emit_insn(i_type(opc, rd, f3, rs1, 0), Some((k, e)), stmt),
        }
        Ok(())
    }

    fn s_with(&mut self, op: Op, rs1: u8, rs2: u8, imm: Imm, stmt: &Stmt) -> Result<(), OpError> {
        let (opc, f3, _) = op_fields(op);
        match self.resolve12(imm, true)? {
            Resolved::Val(v) => self.emit_insn(s_type(opc, f3, rs1, rs2, v), None, stmt),
            Resolved::Fix(k, e) => self.emit_insn(s_type(opc, f3, rs1, rs2, 0), Some((k, e)), stmt),
        }
        Ok(())
    }

    fn upper(&mut self, op: Op, rd: u8, imm: Imm, stmt: &Stmt) -> Result<(), OpError> {
        let opc = op_fields(op).0;
        let range_msg = if op == Op::Lui {
            "operand must be a symbol with a %hi modifier or an integer in the range [0, 1048575]"
        } else {
            "operand must be a symbol with a %pcrel_hi modifier or an integer in the range [0, 1048575]"
        };
        let (field, fix) = match imm {
            Imm::Plain(e, col) => match self.eval_now(&e).map_err(|m| (col, m))? {
                Some(Value::Abs(v)) if (0..1 << 20).contains(&v) => (v as u32, None),
                Some(_) => return Err((col, range_msg.into())),
                None => (0, Some((FixKind::Imm20, e))),
            },
            Imm::Reloc(RelocOp::Hi, e, col) if op == Op::Lui => {
                match self.eval_now(&e).map_err(|m| (col, m))? {
                    Some(Value::Abs(v)) => (hi20(v), None),
                    _ => (0, Some((FixKind::Hi20, e))),
                }
            }
            Imm::Reloc(RelocOp::PcrelHi, e, _) if op == Op::Auipc => {
                (0, Some((FixKind::PcrelHi20, e)))
            }
            Imm::Reloc(_, _, col) => return Err((col, range_msg.into())),
        };
        self.emit_insn(u_type(opc, rd, (field << 12) as i32), fix, stmt);
        Ok(())
    }

    fn branch(
        &mut self,
        op: Op,
        rs1: u8,
        rs2: u8,
        target: &[Token],
        stmt: &Stmt,
    ) -> Result<(), OpError> {
        not_reg(target)?;
        let (opc, f3, _) = op_fields(op);
        let col = target[0].col;
        let e = self.parse_expr(stmt, target)?;
        // A value known now (a number) is the offset itself, as in LLVM;
        // anything else is a target address resolved at link time.
        match self.eval_now(&e).map_err(|m| (col, m))? {
            Some(Value::Abs(v)) => {
                if !(-4096..=4094).contains(&v) || v % 2 != 0 {
                    return Err((
                        col,
                        "immediate must be a multiple of 2 bytes in the range [-4096, 4094]".into(),
                    ));
                }
                self.emit_insn(b_type(opc, f3, rs1, rs2, v as i32), None, stmt);
            }
            _ => self.emit_insn(
                b_type(opc, f3, rs1, rs2, 0),
                Some((FixKind::Branch, e)),
                stmt,
            ),
        }
        Ok(())
    }

    fn jal(&mut self, rd: u8, target: &[Token], stmt: &Stmt) -> Result<(), OpError> {
        not_reg(target)?;
        let opc = op_fields(Op::Jal).0;
        let col = target[0].col;
        let e = self.parse_expr(stmt, target)?;
        match self.eval_now(&e).map_err(|m| (col, m))? {
            Some(Value::Abs(v)) => {
                if !(-(1 << 20)..(1 << 20)).contains(&v) || v % 2 != 0 {
                    return Err((
                        col,
                        "immediate must be a multiple of 2 bytes in the range [-1048576, 1048574]"
                            .into(),
                    ));
                }
                self.emit_insn(j_type(opc, rd, v as i32), None, stmt);
            }
            _ => self.emit_insn(j_type(opc, rd, 0), Some((FixKind::Jal, e)), stmt),
        }
        Ok(())
    }

    /// `jalr`/`jr` with one remaining operand: `rs` or `offset(rs)`.
    fn jalr_mem_or_reg(&mut self, rd: u8, op: &[Token], stmt: &Stmt) -> Result<(), OpError> {
        if is_mem(op) {
            let (imm, rs1) = self.mem(stmt, op)?;
            self.i_with(Op::Jalr, rd, rs1, imm, stmt)
        } else {
            let rs1 = reg(op)?;
            self.i_val(Op::Jalr, rd, rs1, 0, stmt);
            Ok(())
        }
    }

    /// `auipc rt, %pcrel_hi(target)` followed by an instruction using
    /// `%pcrel_lo` of that `auipc`.
    fn pcrel_pair(
        &mut self,
        rt: u8,
        target: Expr,
        second: impl FnOnce(u64) -> PairSecond,
        stmt: &Stmt,
    ) {
        let hi_off = self.cur_off();
        self.emit_insn(
            u_type(op_fields(Op::Auipc).0, rt, 0),
            Some((FixKind::PcrelHi20, target)),
            stmt,
        );
        let dummy = Expr::Num(0);
        match second(hi_off) {
            PairSecond::I(op, rd, rs1, off) => {
                let (opc, f3, _) = op_fields(op);
                self.emit_insn(
                    i_type(opc, rd, f3, rs1, 0),
                    Some((FixKind::PcrelLo12I(Some(off)), dummy)),
                    stmt,
                );
            }
            PairSecond::S(op, rs1, rs2, off) => {
                let (opc, f3, _) = op_fields(op);
                self.emit_insn(
                    s_type(opc, f3, rs1, rs2, 0),
                    Some((FixKind::PcrelLo12S(Some(off)), dummy)),
                    stmt,
                );
            }
        }
    }
}

/// Second instruction of a `%pcrel_hi`/`%pcrel_lo` pair.
enum PairSecond {
    /// I-type: op, rd, rs1, offset of the `auipc`.
    I(Op, u8, u8, u64),
    /// S-type: op, rs1 (base), rs2 (data), offset of the `auipc`.
    S(Op, u8, u8, u64),
}

/// Operands of one instruction statement.
struct Ctx<'c> {
    stmt: &'c Stmt,
    head: &'c Token,
    ops: &'c [&'c [Token]],
}

impl Ctx<'_> {
    /// Checks the operand count.
    fn count(&self, n: usize) -> Result<(), OpError> {
        use std::cmp::Ordering::*;
        match self.ops.len().cmp(&n) {
            Equal => Ok(()),
            Less => {
                let col = self
                    .ops
                    .last()
                    .and_then(|o| o.last())
                    .map_or(self.head.col + self.head.len, |t| t.col + t.len);
                Err((
                    col,
                    format!("too few operands for instruction (expected {n})"),
                ))
            }
            Greater => Err((
                self.ops[n][0].col,
                format!("too many operands for instruction (expected {n})"),
            )),
        }
    }
}

/// Parses a register operand.
fn reg(op: &[Token]) -> Result<u8, OpError> {
    match op {
        [t] => match t.ident().and_then(reg_by_name) {
            Some(r) => Ok(r),
            None => Err((t.col, format!("expected a register, found {}", t.tok))),
        },
        [] => Err((0, "expected a register".into())),
        [t, ..] => Err((t.col, "expected a register".into())),
    }
}

/// True if the operand is a single register name.
fn is_reg(op: &[Token]) -> bool {
    matches!(op, [t] if t.ident().and_then(reg_by_name).is_some())
}

/// Rejects a register where an immediate or address is expected.
fn not_reg(op: &[Token]) -> Result<(), OpError> {
    if is_reg(op) {
        return Err((
            op[0].col,
            format!(
                "expected an immediate or symbol, found register {}",
                op[0].tok
            ),
        ));
    }
    Ok(())
}

/// The immediate form of a register CSR instruction.
fn csr_imm_form(op: Op) -> Op {
    match op {
        Op::Csrrw => Op::Csrrwi,
        Op::Csrrs => Op::Csrrsi,
        _ => Op::Csrrci,
    }
}

/// True if the operand ends with `(register)`.
fn is_mem(op: &[Token]) -> bool {
    let n = op.len();
    n >= 3
        && op[n - 1].is_punct(Punct::RParen)
        && op[n - 3].is_punct(Punct::LParen)
        && op[n - 2].ident().and_then(reg_by_name).is_some()
        && balanced(&op[..n - 3])
}

/// True if the operand has the shape `offset(name)` where `name` is an
/// identifier that may or may not be a register (for diagnostics).
fn looks_like_mem(op: &[Token]) -> bool {
    let n = op.len();
    n >= 4
        && op[n - 1].is_punct(Punct::RParen)
        && op[n - 3].is_punct(Punct::LParen)
        && op[n - 2].ident().is_some()
        && !op[n - 4].is_punct(Punct::Percent)
        && balanced(&op[..n - 3])
        && !op[..n - 3].iter().any(|t| t.is_punct(Punct::LParen))
}

/// True if parentheses in `toks` are balanced.
fn balanced(toks: &[Token]) -> bool {
    let mut depth = 0i32;
    for t in toks {
        match t.tok {
            Tok::Punct(Punct::LParen) => depth += 1,
            Tok::Punct(Punct::RParen) => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// Parses a FENCE predecessor/successor set: letters of `iorw` in order, or `0`.
fn fence_set(op: &[Token]) -> Result<u32, OpError> {
    let err = |col| {
        Err((
            col,
            "operand must be formed of letters selected in-order from 'iorw' or be 0".into(),
        ))
    };
    match op {
        [Token {
            tok: Tok::Int(0), ..
        }] => Ok(0),
        [t] => {
            let Some(s) = t.ident() else {
                return err(t.col);
            };
            let mut bits = 0;
            let mut last = 0;
            for ch in s.to_ascii_lowercase().chars() {
                let (bit, order) = match ch {
                    'i' => (8, 1),
                    'o' => (4, 2),
                    'r' => (2, 3),
                    'w' => (1, 4),
                    _ => return err(t.col),
                };
                if order <= last {
                    return err(t.col);
                }
                last = order;
                bits |= bit;
            }
            Ok(bits)
        }
        _ => err(op.first().map_or(0, |t| t.col)),
    }
}
