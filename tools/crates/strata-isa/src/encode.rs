//! Instruction encoding: bit-level format packers, immediate extraction and
//! insertion (used by the assembler to apply relocations), and the checked
//! [`encode`] of an [`Insn`].

use crate::{Format, Insn, Op};
use std::fmt;

/// Major opcodes (`word[6:0]`) used by Strata.
pub mod opcode {
    /// Loads.
    pub const LOAD: u32 = 0x03;
    /// `fence`, `fence.i`.
    pub const MISC_MEM: u32 = 0x0F;
    /// Register-immediate ALU operations.
    pub const OP_IMM: u32 = 0x13;
    /// `auipc`.
    pub const AUIPC: u32 = 0x17;
    /// Stores.
    pub const STORE: u32 = 0x23;
    /// Register-register ALU operations.
    pub const OP: u32 = 0x33;
    /// `lui`.
    pub const LUI: u32 = 0x37;
    /// Conditional branches.
    pub const BRANCH: u32 = 0x63;
    /// `jalr`.
    pub const JALR: u32 = 0x67;
    /// `jal`.
    pub const JAL: u32 = 0x6F;
    /// `ecall`, `ebreak`, `mret`, `wfi`, CSR instructions.
    pub const SYSTEM: u32 = 0x73;
}

/// Packs an R-type instruction.
pub const fn r_type(opcode: u32, rd: u8, funct3: u32, rs1: u8, rs2: u8, funct7: u32) -> u32 {
    (funct7 & 0x7F) << 25
        | (rs2 as u32 & 31) << 20
        | (rs1 as u32 & 31) << 15
        | (funct3 & 7) << 12
        | (rd as u32 & 31) << 7
        | (opcode & 0x7F)
}

/// Packs an I-type instruction; only the low 12 bits of `imm` are used.
pub const fn i_type(opcode: u32, rd: u8, funct3: u32, rs1: u8, imm: i32) -> u32 {
    (imm as u32 & 0xFFF) << 20
        | (rs1 as u32 & 31) << 15
        | (funct3 & 7) << 12
        | (rd as u32 & 31) << 7
        | (opcode & 0x7F)
}

/// Packs an S-type instruction; only the low 12 bits of `imm` are used.
pub const fn s_type(opcode: u32, funct3: u32, rs1: u8, rs2: u8, imm: i32) -> u32 {
    with_s_imm(
        (rs2 as u32 & 31) << 20 | (rs1 as u32 & 31) << 15 | (funct3 & 7) << 12 | (opcode & 0x7F),
        imm,
    )
}

/// Packs a B-type instruction; `imm` is the byte offset (bits 12:1 used).
pub const fn b_type(opcode: u32, funct3: u32, rs1: u8, rs2: u8, imm: i32) -> u32 {
    with_b_imm(
        (rs2 as u32 & 31) << 20 | (rs1 as u32 & 31) << 15 | (funct3 & 7) << 12 | (opcode & 0x7F),
        imm,
    )
}

/// Packs a U-type instruction; `imm` is the upper value (bits 31:12 used).
pub const fn u_type(opcode: u32, rd: u8, imm: i32) -> u32 {
    (imm as u32 & 0xFFFF_F000) | (rd as u32 & 31) << 7 | (opcode & 0x7F)
}

/// Packs a J-type instruction; `imm` is the byte offset (bits 20:1 used).
pub const fn j_type(opcode: u32, rd: u8, imm: i32) -> u32 {
    with_j_imm((rd as u32 & 31) << 7 | (opcode & 0x7F), imm)
}

/// Sign-extended I-type immediate of `word`.
pub const fn i_imm(word: u32) -> i32 {
    (word as i32) >> 20
}

/// Sign-extended S-type immediate of `word`.
pub const fn s_imm(word: u32) -> i32 {
    ((word as i32) >> 25) << 5 | ((word >> 7) & 0x1F) as i32
}

/// Sign-extended B-type byte offset of `word`.
pub const fn b_imm(word: u32) -> i32 {
    ((word as i32) >> 31) << 12
        | (((word >> 7) & 1) << 11) as i32
        | (((word >> 25) & 0x3F) << 5) as i32
        | (((word >> 8) & 0xF) << 1) as i32
}

/// U-type upper immediate of `word` (`word & 0xFFFF_F000`).
pub const fn u_imm(word: u32) -> i32 {
    (word & 0xFFFF_F000) as i32
}

/// Sign-extended J-type byte offset of `word`.
pub const fn j_imm(word: u32) -> i32 {
    ((word as i32) >> 31) << 20
        | (word & 0x000F_F000) as i32
        | (((word >> 20) & 1) << 11) as i32
        | (((word >> 21) & 0x3FF) << 1) as i32
}

/// Replaces the I-type immediate field of `word` with the low 12 bits of `imm`.
pub const fn with_i_imm(word: u32, imm: i32) -> u32 {
    (word & 0x000F_FFFF) | (imm as u32 & 0xFFF) << 20
}

/// Replaces the S-type immediate fields of `word` with the low 12 bits of `imm`.
pub const fn with_s_imm(word: u32, imm: i32) -> u32 {
    let imm = imm as u32;
    (word & 0x01FF_F07F) | (imm & 0xFE0) << 20 | (imm & 0x1F) << 7
}

/// Replaces the B-type offset fields of `word` with bits 12:1 of `imm`.
pub const fn with_b_imm(word: u32, imm: i32) -> u32 {
    let imm = imm as u32;
    (word & 0x01FF_F07F)
        | ((imm >> 12) & 1) << 31
        | ((imm >> 5) & 0x3F) << 25
        | ((imm >> 1) & 0xF) << 8
        | ((imm >> 11) & 1) << 7
}

/// Replaces the U-type immediate of `word` with bits 31:12 of `imm`.
pub const fn with_u_imm(word: u32, imm: i32) -> u32 {
    (word & 0xFFF) | (imm as u32 & 0xFFFF_F000)
}

/// Replaces the J-type offset fields of `word` with bits 20:1 of `imm`.
pub const fn with_j_imm(word: u32, imm: i32) -> u32 {
    let imm = imm as u32;
    (word & 0xFFF)
        | ((imm >> 20) & 1) << 31
        | ((imm >> 1) & 0x3FF) << 21
        | ((imm >> 11) & 1) << 20
        | (imm & 0x000F_F000)
}

/// True if `v` is representable as a `bits`-bit two's-complement number.
pub const fn fits_signed(v: i64, bits: u32) -> bool {
    let min = -(1i64 << (bits - 1));
    let max = (1i64 << (bits - 1)) - 1;
    v >= min && v <= max
}

/// True if `v` is representable as a `bits`-bit unsigned number.
pub const fn fits_unsigned(v: i64, bits: u32) -> bool {
    v >= 0 && v < (1i64 << bits)
}

/// `%hi` part of a 32-bit value: the 20-bit field such that
/// `(hi << 12) + sext(lo12(value)) == value` (mod 2^32).
pub const fn hi20(value: i64) -> u32 {
    ((value.wrapping_add(0x800) >> 12) as u32) & 0xF_FFFF
}

/// `%lo` part of a 32-bit value: its low 12 bits, sign-extended.
pub const fn lo12(value: i64) -> i32 {
    ((value as i32) << 20) >> 20
}

/// Why an [`Insn`] could not be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// A register number above 31.
    Register(u8),
    /// An immediate outside the field's range.
    Immediate {
        /// The operation being encoded.
        op: Op,
        /// The offending immediate.
        imm: i32,
        /// Smallest allowed value.
        min: i64,
        /// Largest allowed value.
        max: i64,
    },
    /// A branch/jump offset that is odd, or an upper immediate with low bits set.
    Alignment {
        /// The operation being encoded.
        op: Op,
        /// The offending immediate.
        imm: i32,
    },
    /// A CSR address above `0xFFF`.
    Csr(u16),
    /// [`Op::Illegal`] has no encoding.
    Illegal,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::Register(r) => write!(f, "invalid register number {r}"),
            EncodeError::Immediate { op, imm, min, max } => {
                write!(f, "immediate {imm} out of range for {op} [{min}, {max}]")
            }
            EncodeError::Alignment { op, imm } => {
                write!(f, "misaligned immediate {imm:#x} for {op}")
            }
            EncodeError::Csr(c) => write!(f, "invalid CSR address {c:#x}"),
            EncodeError::Illegal => write!(f, "illegal instruction has no encoding"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// `(opcode, funct3, funct7)` of an operation. `funct7` is meaningful for
/// R-type and shift operations only.
pub fn op_fields(op: Op) -> (u32, u32, u32) {
    use opcode::*;
    use Op::*;
    match op {
        Lui => (LUI, 0, 0),
        Auipc => (AUIPC, 0, 0),
        Jal => (JAL, 0, 0),
        Jalr => (JALR, 0, 0),
        Beq => (BRANCH, 0, 0),
        Bne => (BRANCH, 1, 0),
        Blt => (BRANCH, 4, 0),
        Bge => (BRANCH, 5, 0),
        Bltu => (BRANCH, 6, 0),
        Bgeu => (BRANCH, 7, 0),
        Lb => (LOAD, 0, 0),
        Lh => (LOAD, 1, 0),
        Lw => (LOAD, 2, 0),
        Lbu => (LOAD, 4, 0),
        Lhu => (LOAD, 5, 0),
        Sb => (STORE, 0, 0),
        Sh => (STORE, 1, 0),
        Sw => (STORE, 2, 0),
        Addi => (OP_IMM, 0, 0),
        Slti => (OP_IMM, 2, 0),
        Sltiu => (OP_IMM, 3, 0),
        Xori => (OP_IMM, 4, 0),
        Ori => (OP_IMM, 6, 0),
        Andi => (OP_IMM, 7, 0),
        Slli => (OP_IMM, 1, 0),
        Srli => (OP_IMM, 5, 0),
        Srai => (OP_IMM, 5, 0x20),
        Add => (OP, 0, 0),
        Sub => (OP, 0, 0x20),
        Sll => (OP, 1, 0),
        Slt => (OP, 2, 0),
        Sltu => (OP, 3, 0),
        Xor => (OP, 4, 0),
        Srl => (OP, 5, 0),
        Sra => (OP, 5, 0x20),
        Or => (OP, 6, 0),
        And => (OP, 7, 0),
        Fence => (MISC_MEM, 0, 0),
        FenceI => (MISC_MEM, 1, 0),
        Ecall | Ebreak | Mret | Wfi => (SYSTEM, 0, 0),
        Csrrw => (SYSTEM, 1, 0),
        Csrrs => (SYSTEM, 2, 0),
        Csrrc => (SYSTEM, 3, 0),
        Csrrwi => (SYSTEM, 5, 0),
        Csrrsi => (SYSTEM, 6, 0),
        Csrrci => (SYSTEM, 7, 0),
        Illegal => (0, 0, 0),
    }
}

fn check_range(op: Op, imm: i32, min: i64, max: i64) -> Result<(), EncodeError> {
    if (min..=max).contains(&i64::from(imm)) {
        Ok(())
    } else {
        Err(EncodeError::Immediate { op, imm, min, max })
    }
}

/// Encodes `insn`, checking that every field fits.
///
/// Fields the format does not use are ignored. This is the inverse of
/// [`decode`](crate::decode) for every legal word whose ignored fields are zero.
pub fn encode(insn: &Insn) -> Result<u32, EncodeError> {
    let Insn {
        op,
        rd,
        rs1,
        rs2,
        imm,
        csr,
    } = *insn;
    let format = op.format();
    let uses = |r: u8, used: bool| {
        if used && r > 31 {
            Err(EncodeError::Register(r))
        } else {
            Ok(())
        }
    };
    uses(rd, op.has_rd())?;
    uses(rs1, op.reads_rs1())?;
    uses(rs2, op.reads_rs2())?;
    if op.is_csr() && csr > 0xFFF {
        return Err(EncodeError::Csr(csr));
    }
    let (opc, f3, f7) = op_fields(op);
    let word = match format {
        Format::R => r_type(opc, rd, f3, rs1, rs2, f7),
        Format::I | Format::Load => {
            check_range(op, imm, -2048, 2047)?;
            i_type(opc, rd, f3, rs1, imm)
        }
        Format::Shift => {
            check_range(op, imm, 0, 31)?;
            i_type(opc, rd, f3, rs1, imm | (f7 << 5) as i32)
        }
        Format::S => {
            check_range(op, imm, -2048, 2047)?;
            s_type(opc, f3, rs1, rs2, imm)
        }
        Format::B => {
            check_range(op, imm, -4096, 4094)?;
            if imm & 1 != 0 {
                return Err(EncodeError::Alignment { op, imm });
            }
            b_type(opc, f3, rs1, rs2, imm)
        }
        Format::U => {
            if imm & 0xFFF != 0 {
                return Err(EncodeError::Alignment { op, imm });
            }
            u_type(opc, rd, imm)
        }
        Format::J => {
            check_range(op, imm, -(1 << 20), (1 << 20) - 2)?;
            if imm & 1 != 0 {
                return Err(EncodeError::Alignment { op, imm });
            }
            j_type(opc, rd, imm)
        }
        Format::Csr => i_type(opc, rd, f3, rs1, i32::from(csr)),
        Format::CsrI => {
            check_range(op, imm, 0, 31)?;
            i_type(opc, rd, f3, imm as u8, i32::from(csr))
        }
        Format::Fence => {
            check_range(op, imm, 0, 0xFFF)?;
            i_type(opc, 0, f3, 0, imm)
        }
        Format::None => match op {
            Op::FenceI => i_type(opc, 0, f3, 0, 0),
            Op::Ecall => 0x0000_0073,
            Op::Ebreak => 0x0010_0073,
            Op::Mret => 0x3020_0073,
            Op::Wfi => 0x1050_0073,
            _ => unreachable!("format None covers only the operations above"),
        },
        Format::Illegal => return Err(EncodeError::Illegal),
    };
    Ok(word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;

    #[test]
    fn immediate_helpers_round_trip() {
        for imm in [-4096, -4094, -2, 0, 2, 0x7FE, 0x800, 4094] {
            assert_eq!(b_imm(with_b_imm(0x63, imm)), imm);
        }
        for imm in [-(1 << 20), -2, 0, 2, 0x7FE, 0x800, 0xFF000, (1 << 20) - 2] {
            assert_eq!(j_imm(with_j_imm(0x6F, imm)), imm);
        }
        for imm in [-2048, -1, 0, 1, 2047] {
            assert_eq!(i_imm(with_i_imm(0x13, imm)), imm);
            assert_eq!(s_imm(with_s_imm(0x23, imm)), imm);
        }
        assert_eq!(u_imm(with_u_imm(0x37, 0x12345678)), 0x12345000);
    }

    #[test]
    fn hi_lo_split() {
        for v in [
            0i64,
            1,
            0x7FF,
            0x800,
            0xFFF,
            0x1000,
            0x12345678,
            0x7FFFF800,
            -1,
            -2048,
            -2049,
            0xFFFF_FFFF,
        ] {
            let hi = hi20(v);
            let lo = lo12(v);
            let recombined = (hi << 12).wrapping_add(lo as u32);
            assert_eq!(recombined, v as u32, "value {v:#x}");
        }
        assert_eq!(hi20(0x800), 1);
        assert_eq!(lo12(0x800), -2048);
    }

    #[test]
    fn known_encodings() {
        let addi = Insn {
            op: Op::Addi,
            rd: 10,
            rs1: 11,
            imm: -5,
            ..Insn::new(Op::Addi)
        };
        assert_eq!(encode(&addi), Ok(0xffb58513));
        let srai = Insn {
            op: Op::Srai,
            rd: 10,
            rs1: 10,
            imm: 3,
            ..Insn::new(Op::Srai)
        };
        assert_eq!(encode(&srai), Ok(0x40355513));
        let csrr = Insn {
            op: Op::Csrrs,
            rd: 10,
            csr: 0x300,
            ..Insn::new(Op::Csrrs)
        };
        assert_eq!(encode(&csrr), Ok(0x30002573));
        let fence = Insn {
            imm: 0x0FF,
            ..Insn::new(Op::Fence)
        };
        assert_eq!(encode(&fence), Ok(0x0ff0000f));
        assert_eq!(encode(&Insn::new(Op::Wfi)), Ok(0x10500073));
        assert_eq!(encode(&Insn::ILLEGAL), Err(EncodeError::Illegal));
    }

    #[test]
    fn range_errors() {
        let mut i = Insn::new(Op::Addi);
        i.imm = 2048;
        assert!(matches!(encode(&i), Err(EncodeError::Immediate { .. })));
        let mut b = Insn::new(Op::Beq);
        b.imm = 3;
        assert!(matches!(encode(&b), Err(EncodeError::Alignment { .. })));
        b.imm = 4096;
        assert!(matches!(encode(&b), Err(EncodeError::Immediate { .. })));
        let mut j = Insn::new(Op::Jal);
        j.imm = 1 << 20;
        assert!(encode(&j).is_err());
        let mut s = Insn::new(Op::Slli);
        s.imm = 32;
        assert!(encode(&s).is_err());
        let mut r = Insn::new(Op::Add);
        r.rd = 32;
        assert_eq!(encode(&r), Err(EncodeError::Register(32)));
        let mut c = Insn::new(Op::Csrrw);
        c.csr = 0x1000;
        assert_eq!(encode(&c), Err(EncodeError::Csr(0x1000)));
    }

    /// Deterministic xorshift generator so the test needs no dependencies.
    fn xorshift(state: &mut u64) -> u32 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        (*state >> 16) as u32
    }

    #[test]
    fn decode_encode_round_trip() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut legal = 0;
        for _ in 0..400_000 {
            let w = xorshift(&mut state) | 3;
            let insn = decode(w);
            if insn.op == Op::Illegal {
                continue;
            }
            legal += 1;
            let re = encode(&insn).expect("decoded instruction must encode");
            assert_eq!(decode(re), insn, "word {w:#010x}");
            // Only FENCE/FENCE.I have ignored fields; everything else is exact.
            if !matches!(insn.op, Op::Fence | Op::FenceI) {
                assert_eq!(re, w, "word {w:#010x} ({insn:?})");
            }
        }
        assert!(legal > 10_000);
    }
}
