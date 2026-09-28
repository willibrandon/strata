//! Instruction set of the Strata S1 CPU: RV32I, Zicsr, Zifencei, plus `MRET`
//! and `WFI`, exactly as specified in `docs/isa.md`.
//!
//! This crate is shared by the assembler (`strata-asm`) and the debugger. It
//! provides:
//!
//! * [`decode`]: 32-bit word to [`Insn`], deciding legality per `docs/isa.md`
//!   (CSR *address* legality depends on the privilege mode and is left to the
//!   executor; see [`csr_exists`]).
//! * [`encode()`] and the bit-level helpers in [`encode`](mod@encode): [`Insn`]
//!   (or raw fields) to a 32-bit word, with range checking.
//! * [`disassemble`] / [`disassemble_with`]: text in the style of
//!   `llvm-objdump` (ABI register names, canonical aliases, absolute targets).
//! * Register and CSR name tables.
//!
//! # Field conventions of [`Insn`]
//!
//! | Operation class | `rd` | `rs1` | `rs2` | `imm` | `csr` |
//! |---|---|---|---|---|---|
//! | R-type (`add` ...) | dest | src | src | 0 | 0 |
//! | I-type ALU, loads, `jalr` | dest | base/src | 0 | sign-extended imm12 | 0 |
//! | shifts by immediate | dest | src | 0 | shift amount 0..=31 | 0 |
//! | stores | 0 | base | data | sign-extended imm12 | 0 |
//! | branches | 0 | src | src | byte offset from the branch | 0 |
//! | `jal` | dest | 0 | 0 | byte offset from the `jal` | 0 |
//! | `lui`, `auipc` | dest | 0 | 0 | `word & 0xFFFF_F000` (as `i32`) | 0 |
//! | `csrrw`/`csrrs`/`csrrc` | dest | src | 0 | 0 | CSR address |
//! | `csrrwi`/`csrrsi`/`csrrci` | dest | 0 | 0 | `uimm` 0..=31 | CSR address |
//! | `fence` | 0 | 0 | 0 | `fm:pred:succ` field, 0..=4095 | 0 |
//! | `fence.i`, `ecall`, `ebreak`, `mret`, `wfi`, illegal | 0 | 0 | 0 | 0 | 0 |
//!
//! `FENCE` and `FENCE.I` ignore their `rd`/`rs1` (and, for `FENCE.I`, `imm`)
//! fields, so [`decode`] reports them as zero.

#![warn(missing_docs)]

mod csr;
mod decode;
mod disasm;
pub mod encode;
mod reg;

pub use csr::{all_csrs, csr_by_name, csr_exists, csr_is_read_only, csr_name, csr_privilege};
pub use decode::decode;
pub use disasm::{disassemble, disassemble_with, fence_set_name};
pub use encode::{encode, EncodeError};
pub use reg::{reg_by_name, reg_name};

/// An operation of the Strata instruction set, or [`Op::Illegal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Op {
    /// `lui rd, imm`: load upper immediate.
    Lui,
    /// `auipc rd, imm`: add upper immediate to `pc`.
    Auipc,
    /// `jal rd, offset`: jump and link.
    Jal,
    /// `jalr rd, imm(rs1)`: jump and link register.
    Jalr,
    /// Branch if equal.
    Beq,
    /// Branch if not equal.
    Bne,
    /// Branch if less than (signed).
    Blt,
    /// Branch if greater or equal (signed).
    Bge,
    /// Branch if less than (unsigned).
    Bltu,
    /// Branch if greater or equal (unsigned).
    Bgeu,
    /// Load byte, sign-extended.
    Lb,
    /// Load halfword, sign-extended.
    Lh,
    /// Load word.
    Lw,
    /// Load byte, zero-extended.
    Lbu,
    /// Load halfword, zero-extended.
    Lhu,
    /// Store byte.
    Sb,
    /// Store halfword.
    Sh,
    /// Store word.
    Sw,
    /// Add immediate.
    Addi,
    /// Set if less than immediate (signed).
    Slti,
    /// Set if less than immediate (unsigned).
    Sltiu,
    /// Exclusive-or immediate.
    Xori,
    /// Or immediate.
    Ori,
    /// And immediate.
    Andi,
    /// Shift left logical by immediate.
    Slli,
    /// Shift right logical by immediate.
    Srli,
    /// Shift right arithmetic by immediate.
    Srai,
    /// Add.
    Add,
    /// Subtract.
    Sub,
    /// Shift left logical.
    Sll,
    /// Set if less than (signed).
    Slt,
    /// Set if less than (unsigned).
    Sltu,
    /// Exclusive or.
    Xor,
    /// Shift right logical.
    Srl,
    /// Shift right arithmetic.
    Sra,
    /// Or.
    Or,
    /// And.
    And,
    /// `fence pred, succ` (executes as a no-op on Strata).
    Fence,
    /// `fence.i`: synchronize instruction fetches with prior stores.
    FenceI,
    /// Environment call.
    Ecall,
    /// Breakpoint.
    Ebreak,
    /// Return from machine-mode trap.
    Mret,
    /// Wait for interrupt.
    Wfi,
    /// Atomic read/write CSR.
    Csrrw,
    /// Atomic read and set bits in CSR.
    Csrrs,
    /// Atomic read and clear bits in CSR.
    Csrrc,
    /// `csrrw` with a 5-bit immediate source.
    Csrrwi,
    /// `csrrs` with a 5-bit immediate source.
    Csrrsi,
    /// `csrrc` with a 5-bit immediate source.
    Csrrci,
    /// Any word that is not a Strata instruction (raises cause 2).
    Illegal,
}

/// Instruction encoding format, as far as operand layout is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// `op rd, rs1, rs2`
    R,
    /// `op rd, rs1, imm12` (ALU immediates and `jalr`)
    I,
    /// `op rd, rs1, shamt`
    Shift,
    /// `op rd, imm12(rs1)`
    Load,
    /// `op rs2, imm12(rs1)`
    S,
    /// `op rs1, rs2, target`
    B,
    /// `op rd, imm20`
    U,
    /// `jal rd, target`
    J,
    /// `op rd, csr, rs1`
    Csr,
    /// `op rd, csr, uimm5`
    CsrI,
    /// `fence pred, succ`
    Fence,
    /// No operands (`fence.i`, `ecall`, `ebreak`, `mret`, `wfi`).
    None,
    /// Not an instruction.
    Illegal,
}

impl Op {
    /// Every legal operation (excludes [`Op::Illegal`]), in encoding-table order.
    pub const ALL: [Op; 49] = [
        Op::Lui,
        Op::Auipc,
        Op::Jal,
        Op::Jalr,
        Op::Beq,
        Op::Bne,
        Op::Blt,
        Op::Bge,
        Op::Bltu,
        Op::Bgeu,
        Op::Lb,
        Op::Lh,
        Op::Lw,
        Op::Lbu,
        Op::Lhu,
        Op::Sb,
        Op::Sh,
        Op::Sw,
        Op::Addi,
        Op::Slti,
        Op::Sltiu,
        Op::Xori,
        Op::Ori,
        Op::Andi,
        Op::Slli,
        Op::Srli,
        Op::Srai,
        Op::Add,
        Op::Sub,
        Op::Sll,
        Op::Slt,
        Op::Sltu,
        Op::Xor,
        Op::Srl,
        Op::Sra,
        Op::Or,
        Op::And,
        Op::Fence,
        Op::FenceI,
        Op::Ecall,
        Op::Ebreak,
        Op::Mret,
        Op::Wfi,
        Op::Csrrw,
        Op::Csrrs,
        Op::Csrrc,
        Op::Csrrwi,
        Op::Csrrsi,
        Op::Csrrci,
    ];

    /// The base (non-alias) assembler mnemonic, e.g. `"addi"` or `"fence.i"`.
    pub fn mnemonic(self) -> &'static str {
        use Op::*;
        match self {
            Lui => "lui",
            Auipc => "auipc",
            Jal => "jal",
            Jalr => "jalr",
            Beq => "beq",
            Bne => "bne",
            Blt => "blt",
            Bge => "bge",
            Bltu => "bltu",
            Bgeu => "bgeu",
            Lb => "lb",
            Lh => "lh",
            Lw => "lw",
            Lbu => "lbu",
            Lhu => "lhu",
            Sb => "sb",
            Sh => "sh",
            Sw => "sw",
            Addi => "addi",
            Slti => "slti",
            Sltiu => "sltiu",
            Xori => "xori",
            Ori => "ori",
            Andi => "andi",
            Slli => "slli",
            Srli => "srli",
            Srai => "srai",
            Add => "add",
            Sub => "sub",
            Sll => "sll",
            Slt => "slt",
            Sltu => "sltu",
            Xor => "xor",
            Srl => "srl",
            Sra => "sra",
            Or => "or",
            And => "and",
            Fence => "fence",
            FenceI => "fence.i",
            Ecall => "ecall",
            Ebreak => "ebreak",
            Mret => "mret",
            Wfi => "wfi",
            Csrrw => "csrrw",
            Csrrs => "csrrs",
            Csrrc => "csrrc",
            Csrrwi => "csrrwi",
            Csrrsi => "csrrsi",
            Csrrci => "csrrci",
            Illegal => "illegal",
        }
    }

    /// Looks up an operation by its base mnemonic (case-insensitive).
    /// Aliases such as `mv` or `csrr` are not recognized here.
    pub fn from_mnemonic(name: &str) -> Option<Op> {
        let lower = name.to_ascii_lowercase();
        Op::ALL.iter().copied().find(|op| op.mnemonic() == lower)
    }

    /// Operand layout of this operation.
    pub fn format(self) -> Format {
        use Op::*;
        match self {
            Lui | Auipc => Format::U,
            Jal => Format::J,
            Jalr | Addi | Slti | Sltiu | Xori | Ori | Andi => Format::I,
            Beq | Bne | Blt | Bge | Bltu | Bgeu => Format::B,
            Lb | Lh | Lw | Lbu | Lhu => Format::Load,
            Sb | Sh | Sw => Format::S,
            Slli | Srli | Srai => Format::Shift,
            Add | Sub | Sll | Slt | Sltu | Xor | Srl | Sra | Or | And => Format::R,
            Fence => Format::Fence,
            FenceI | Ecall | Ebreak | Mret | Wfi => Format::None,
            Csrrw | Csrrs | Csrrc => Format::Csr,
            Csrrwi | Csrrsi | Csrrci => Format::CsrI,
            Illegal => Format::Illegal,
        }
    }

    /// True for the loads `lb lh lw lbu lhu`.
    pub fn is_load(self) -> bool {
        self.format() == Format::Load
    }

    /// True for the stores `sb sh sw`.
    pub fn is_store(self) -> bool {
        self.format() == Format::S
    }

    /// True for the conditional branches.
    pub fn is_branch(self) -> bool {
        self.format() == Format::B
    }

    /// True for the unconditional jumps `jal` and `jalr`.
    pub fn is_jump(self) -> bool {
        matches!(self, Op::Jal | Op::Jalr)
    }

    /// True for the six Zicsr instructions.
    pub fn is_csr(self) -> bool {
        matches!(self.format(), Format::Csr | Format::CsrI)
    }

    /// Access width in bytes of a load or store, `None` for other operations.
    pub fn mem_width(self) -> Option<u8> {
        use Op::*;
        match self {
            Lb | Lbu | Sb => Some(1),
            Lh | Lhu | Sh => Some(2),
            Lw | Sw => Some(4),
            _ => None,
        }
    }

    /// True if the operation reads register `rs1`.
    pub fn reads_rs1(self) -> bool {
        matches!(
            self.format(),
            Format::R
                | Format::I
                | Format::Shift
                | Format::Load
                | Format::S
                | Format::B
                | Format::Csr
        )
    }

    /// True if the operation reads register `rs2`.
    pub fn reads_rs2(self) -> bool {
        matches!(self.format(), Format::R | Format::S | Format::B)
    }

    /// True if the operation has a destination register field `rd`.
    pub fn has_rd(self) -> bool {
        matches!(
            self.format(),
            Format::R
                | Format::I
                | Format::Shift
                | Format::Load
                | Format::U
                | Format::J
                | Format::Csr
                | Format::CsrI
        )
    }
}

impl std::fmt::Display for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.mnemonic())
    }
}

/// A decoded instruction. See the crate documentation for field conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Insn {
    /// The operation ([`Op::Illegal`] for words that are not instructions).
    pub op: Op,
    /// Destination register number (0-31).
    pub rd: u8,
    /// First source register number (0-31).
    pub rs1: u8,
    /// Second source register number (0-31).
    pub rs2: u8,
    /// Immediate operand; its meaning depends on the format (see the crate docs).
    pub imm: i32,
    /// CSR address (0-0xFFF) of a CSR instruction.
    pub csr: u16,
}

impl Insn {
    /// The result of decoding a word that is not a Strata instruction.
    pub const ILLEGAL: Insn = Insn::new(Op::Illegal);

    /// An instruction with all operand fields zero.
    pub const fn new(op: Op) -> Insn {
        Insn {
            op,
            rd: 0,
            rs1: 0,
            rs2: 0,
            imm: 0,
            csr: 0,
        }
    }

    /// Base mnemonic of the operation (never an alias).
    pub fn mnemonic(&self) -> &'static str {
        self.op.mnemonic()
    }

    /// True unless the word was illegal.
    pub fn is_legal(&self) -> bool {
        self.op != Op::Illegal
    }

    /// True if the instruction reads `rs1` (register forms only; `x0` counts).
    pub fn reads_rs1(&self) -> bool {
        self.op.reads_rs1()
    }

    /// True if the instruction reads `rs2` (`x0` counts).
    pub fn reads_rs2(&self) -> bool {
        self.op.reads_rs2()
    }

    /// True if the instruction architecturally writes register `rd`, i.e. it
    /// has a destination and `rd != x0`.
    pub fn writes_rd(&self) -> bool {
        self.op.has_rd() && self.rd != 0
    }

    /// See [`Op::is_load`].
    pub fn is_load(&self) -> bool {
        self.op.is_load()
    }

    /// See [`Op::is_store`].
    pub fn is_store(&self) -> bool {
        self.op.is_store()
    }

    /// See [`Op::is_branch`].
    pub fn is_branch(&self) -> bool {
        self.op.is_branch()
    }

    /// See [`Op::is_jump`].
    pub fn is_jump(&self) -> bool {
        self.op.is_jump()
    }

    /// See [`Op::mem_width`].
    pub fn mem_width(&self) -> Option<u8> {
        self.op.mem_width()
    }

    /// For a CSR instruction, whether it writes the CSR (`docs/isa.md`:
    /// `CSRRW[I]` always; `CSRRS`/`CSRRC` iff `rs1 != x0`; `CSRRSI`/`CSRRCI`
    /// iff `uimm != 0`). `false` for other instructions.
    pub fn writes_csr(&self) -> bool {
        match self.op {
            Op::Csrrw | Op::Csrrwi => true,
            Op::Csrrs | Op::Csrrc => self.rs1 != 0,
            Op::Csrrsi | Op::Csrrci => self.imm != 0,
            _ => false,
        }
    }

    /// For a CSR instruction, whether it reads the CSR (`CSRRW[I]` with
    /// `rd = x0` do not). `false` for other instructions.
    pub fn reads_csr(&self) -> bool {
        match self.op {
            Op::Csrrw | Op::Csrrwi => self.rd != 0,
            op => op.is_csr(),
        }
    }

    /// Static control-transfer target of a branch or `jal` at `pc`.
    pub fn target(&self, pc: u32) -> Option<u32> {
        match self.op.format() {
            Format::B | Format::J => Some(pc.wrapping_add(self.imm as u32)),
            _ => None,
        }
    }
}

impl Default for Insn {
    fn default() -> Self {
        Insn::ILLEGAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonics_round_trip() {
        for op in Op::ALL {
            assert_eq!(Op::from_mnemonic(op.mnemonic()), Some(op));
        }
        assert_eq!(Op::from_mnemonic("FENCE.I"), Some(Op::FenceI));
        assert_eq!(Op::from_mnemonic("mv"), None);
        assert_eq!(Op::from_mnemonic("illegal"), None);
    }

    #[test]
    fn classification() {
        assert!(Op::Lhu.is_load() && Op::Lhu.mem_width() == Some(2));
        assert!(Op::Sw.is_store() && Op::Sw.mem_width() == Some(4));
        assert!(Op::Bgeu.is_branch() && !Op::Bgeu.is_jump());
        assert!(Op::Jalr.is_jump() && Op::Jalr.reads_rs1() && !Op::Jalr.reads_rs2());
        assert!(!Op::Csrrwi.reads_rs1() && Op::Csrrw.reads_rs1());
        assert!(!Op::Fence.reads_rs1() && !Op::Fence.has_rd());
        assert!(Op::Sb.reads_rs2() && !Op::Sb.has_rd());
        let mut i = Insn::new(Op::Addi);
        assert!(!i.writes_rd());
        i.rd = 5;
        assert!(i.writes_rd());
        let mut c = Insn::new(Op::Csrrs);
        assert!(!c.writes_csr() && c.reads_csr());
        c.rs1 = 1;
        assert!(c.writes_csr());
        let w = Insn {
            op: Op::Csrrw,
            rd: 0,
            ..Insn::new(Op::Csrrw)
        };
        assert!(w.writes_csr() && !w.reads_csr());
        let b = Insn {
            imm: -8,
            ..Insn::new(Op::Beq)
        };
        assert_eq!(b.target(0x100), Some(0xF8));
        assert_eq!(Insn::new(Op::Add).target(0), None);
    }
}
