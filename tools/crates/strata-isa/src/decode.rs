//! Instruction decoding with Strata's legality rules (`docs/isa.md`).

use crate::encode::{b_imm, i_imm, j_imm, opcode, s_imm, u_imm};
use crate::{Insn, Op};

/// Decodes one 32-bit instruction word.
///
/// Returns [`Insn::ILLEGAL`] for every word that is not exactly one of the
/// Strata encodings: low bits other than `11`, unknown opcodes, reserved
/// `funct3`/`funct7` values, shift immediates with a nonzero `imm[11:5]`
/// (other than `0100000` for `SRAI`), `ECALL`/`EBREAK`/`MRET`/`WFI` with any
/// nonzero unused field, and all other SYSTEM encodings (`SRET`, `DRET`,
/// `SFENCE.VMA`, ...). `FENCE` and `FENCE.I` ignore their `rd`, `rs1` and
/// immediate fields and decode with those fields zeroed (the `FENCE`
/// `fm:pred:succ` field is kept in `imm` for display).
///
/// CSR instructions decode for any CSR address; whether the access is legal
/// depends on the privilege mode (see [`csr_exists`](crate::csr_exists)).
pub fn decode(word: u32) -> Insn {
    if word & 3 != 3 {
        return Insn::ILLEGAL;
    }
    let rd = ((word >> 7) & 31) as u8;
    let funct3 = (word >> 12) & 7;
    let rs1 = ((word >> 15) & 31) as u8;
    let rs2 = ((word >> 20) & 31) as u8;
    let funct7 = word >> 25;
    let base = |op: Op| Insn::new(op);

    match word & 0x7F {
        opcode::LUI => Insn {
            rd,
            imm: u_imm(word),
            ..base(Op::Lui)
        },
        opcode::AUIPC => Insn {
            rd,
            imm: u_imm(word),
            ..base(Op::Auipc)
        },
        opcode::JAL => Insn {
            rd,
            imm: j_imm(word),
            ..base(Op::Jal)
        },
        opcode::JALR if funct3 == 0 => Insn {
            rd,
            rs1,
            imm: i_imm(word),
            ..base(Op::Jalr)
        },
        opcode::BRANCH => {
            let op = match funct3 {
                0 => Op::Beq,
                1 => Op::Bne,
                4 => Op::Blt,
                5 => Op::Bge,
                6 => Op::Bltu,
                7 => Op::Bgeu,
                _ => return Insn::ILLEGAL,
            };
            Insn {
                rs1,
                rs2,
                imm: b_imm(word),
                ..base(op)
            }
        }
        opcode::LOAD => {
            let op = match funct3 {
                0 => Op::Lb,
                1 => Op::Lh,
                2 => Op::Lw,
                4 => Op::Lbu,
                5 => Op::Lhu,
                _ => return Insn::ILLEGAL,
            };
            Insn {
                rd,
                rs1,
                imm: i_imm(word),
                ..base(op)
            }
        }
        opcode::STORE => {
            let op = match funct3 {
                0 => Op::Sb,
                1 => Op::Sh,
                2 => Op::Sw,
                _ => return Insn::ILLEGAL,
            };
            Insn {
                rs1,
                rs2,
                imm: s_imm(word),
                ..base(op)
            }
        }
        opcode::OP_IMM => {
            let op = match (funct3, funct7) {
                (0, _) => Op::Addi,
                (2, _) => Op::Slti,
                (3, _) => Op::Sltiu,
                (4, _) => Op::Xori,
                (6, _) => Op::Ori,
                (7, _) => Op::Andi,
                (1, 0x00) => Op::Slli,
                (5, 0x00) => Op::Srli,
                (5, 0x20) => Op::Srai,
                _ => return Insn::ILLEGAL,
            };
            let imm = if matches!(funct3, 1 | 5) {
                i32::from(rs2)
            } else {
                i_imm(word)
            };
            Insn {
                rd,
                rs1,
                imm,
                ..base(op)
            }
        }
        opcode::OP => {
            let op = match (funct7, funct3) {
                (0x00, 0) => Op::Add,
                (0x20, 0) => Op::Sub,
                (0x00, 1) => Op::Sll,
                (0x00, 2) => Op::Slt,
                (0x00, 3) => Op::Sltu,
                (0x00, 4) => Op::Xor,
                (0x00, 5) => Op::Srl,
                (0x20, 5) => Op::Sra,
                (0x00, 6) => Op::Or,
                (0x00, 7) => Op::And,
                _ => return Insn::ILLEGAL,
            };
            Insn {
                rd,
                rs1,
                rs2,
                ..base(op)
            }
        }
        opcode::MISC_MEM => match funct3 {
            0 => Insn {
                imm: (word >> 20) as i32,
                ..base(Op::Fence)
            },
            1 => base(Op::FenceI),
            _ => Insn::ILLEGAL,
        },
        opcode::SYSTEM => {
            let csr = (word >> 20) as u16;
            match funct3 {
                0 => match word {
                    0x0000_0073 => base(Op::Ecall),
                    0x0010_0073 => base(Op::Ebreak),
                    0x3020_0073 => base(Op::Mret),
                    0x1050_0073 => base(Op::Wfi),
                    _ => Insn::ILLEGAL,
                },
                1 => Insn {
                    rd,
                    rs1,
                    csr,
                    ..base(Op::Csrrw)
                },
                2 => Insn {
                    rd,
                    rs1,
                    csr,
                    ..base(Op::Csrrs)
                },
                3 => Insn {
                    rd,
                    rs1,
                    csr,
                    ..base(Op::Csrrc)
                },
                5 => Insn {
                    rd,
                    imm: i32::from(rs1),
                    csr,
                    ..base(Op::Csrrwi)
                },
                6 => Insn {
                    rd,
                    imm: i32::from(rs1),
                    csr,
                    ..base(Op::Csrrsi)
                },
                7 => Insn {
                    rd,
                    imm: i32::from(rs1),
                    csr,
                    ..base(Op::Csrrci)
                },
                _ => Insn::ILLEGAL,
            }
        }
        _ => Insn::ILLEGAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(w: u32) -> Op {
        decode(w).op
    }

    #[test]
    fn special_words() {
        assert_eq!(op(0x0000_0000), Op::Illegal);
        assert_eq!(op(0xFFFF_FFFF), Op::Illegal);
        assert_eq!(op(0x0000_0013), Op::Addi);
        assert_eq!(op(0x0000_0012), Op::Illegal, "low bits 10");
        assert_eq!(op(0x0000_0011), Op::Illegal, "low bits 01");
        assert_eq!(op(0xC000_1073), Op::Csrrw, "unimp");
    }

    #[test]
    fn system_encodings_are_exact() {
        assert_eq!(op(0x0000_0073), Op::Ecall);
        assert_eq!(op(0x0010_0073), Op::Ebreak);
        assert_eq!(op(0x3020_0073), Op::Mret);
        assert_eq!(op(0x1050_0073), Op::Wfi);
        // Nonzero rd / rs1 / unused fields.
        assert_eq!(op(0x0000_00F3), Op::Illegal);
        assert_eq!(op(0x0000_8073), Op::Illegal);
        assert_eq!(op(0x3020_00F3), Op::Illegal);
        assert_eq!(op(0x1050_8073), Op::Illegal);
        // SRET, DRET, SFENCE.VMA, URET are not implemented.
        assert_eq!(op(0x1020_0073), Op::Illegal);
        assert_eq!(op(0x7B20_0073), Op::Illegal);
        assert_eq!(op(0x1200_0073), Op::Illegal);
        assert_eq!(op(0x0020_0073), Op::Illegal);
        // funct3 = 4 in SYSTEM is reserved.
        assert_eq!(op(0x3000_4073), Op::Illegal);
    }

    #[test]
    fn shift_immediates() {
        assert_eq!(op(0x01F5_1513), Op::Slli);
        assert_eq!(op(0x0215_1513), Op::Illegal, "slli with imm[5] set");
        assert_eq!(op(0x4015_5513), Op::Srai);
        assert_eq!(op(0x4215_5513), Op::Illegal, "srai with imm[5] set");
        assert_eq!(op(0x6015_5513), Op::Illegal, "srai with funct7 0110000");
        assert_eq!(op(0x4015_1513), Op::Illegal, "slli with funct7 0100000");
        assert_eq!(decode(0x4015_5513).imm, 1);
    }

    #[test]
    fn fences_ignore_fields() {
        let f = decode(0x0FF0_000F);
        assert_eq!((f.op, f.imm), (Op::Fence, 0xFF));
        // rd = 5, rs1 = 7: still FENCE, fields reported as zero.
        let f = decode(0x0FF3_828F);
        assert_eq!((f.op, f.rd, f.rs1, f.imm), (Op::Fence, 0, 0, 0xFF));
        assert_eq!(decode(0x8330_000F).imm, 0x833);
        let fi = decode(0xABC3_928F);
        assert_eq!((fi.op, fi.rd, fi.rs1, fi.imm), (Op::FenceI, 0, 0, 0));
        assert_eq!(
            op(0x0000_200F),
            Op::Illegal,
            "MISC-MEM funct3 2 (CBO) is reserved"
        );
    }

    #[test]
    fn reserved_funct_fields() {
        assert_eq!(op(0x0000_2063), Op::Illegal, "branch funct3 2");
        assert_eq!(op(0x0000_3063), Op::Illegal, "branch funct3 3");
        assert_eq!(op(0x0000_3003), Op::Illegal, "load funct3 3 (ld)");
        assert_eq!(op(0x0000_6003), Op::Illegal, "load funct3 6 (lwu)");
        assert_eq!(op(0x0000_3023), Op::Illegal, "store funct3 3 (sd)");
        assert_eq!(op(0x0000_1067), Op::Illegal, "jalr funct3 1");
        assert_eq!(op(0x0200_0033), Op::Illegal, "mul (M extension)");
        assert_eq!(op(0x4000_1033), Op::Illegal, "sll with funct7 0100000");
        assert_eq!(op(0x0000_001B), Op::Illegal, "OP-IMM-32");
    }

    #[test]
    fn immediates() {
        let i = decode(0xFFB5_8513);
        assert_eq!((i.op, i.rd, i.rs1, i.imm), (Op::Addi, 10, 11, -5));
        let s = decode(0x00A1_2423);
        assert_eq!((s.op, s.rs1, s.rs2, s.imm), (Op::Sw, 2, 10, 8));
        let b = decode(0xFC05_0AE3);
        assert_eq!((b.op, b.rs1, b.rs2, b.imm), (Op::Beq, 10, 0, -44));
        let j = decode(0xFB9F_F06F);
        assert_eq!((j.op, j.rd, j.imm), (Op::Jal, 0, -72));
        let u = decode(0xFFFF_F537);
        assert_eq!((u.op, u.rd, u.imm), (Op::Lui, 10, 0xFFFF_F000u32 as i32));
        let c = decode(0x3002_D073);
        assert_eq!((c.op, c.rd, c.imm, c.csr), (Op::Csrrwi, 0, 5, 0x300));
    }
}
