//! Disassembly in the style of `llvm-objdump`: ABI register names, CSR names,
//! the canonical pseudo-instruction aliases, hexadecimal immediates, and
//! branch/jump targets as absolute addresses.

use crate::{csr_name, decode, reg_name, Insn, Op};

/// Disassembles `word` located at address `pc`.
///
/// Illegal words are shown as `.word 0x........`.
pub fn disassemble(word: u32, pc: u32) -> String {
    disassemble_with(word, pc, &|_| None)
}

/// Like [`disassemble`], but branch and jump targets are followed by
/// ` <text>` whenever `symbolize(target)` returns `Some(text)` (typically
/// `"name"` or `"name+0x10"`).
pub fn disassemble_with(word: u32, pc: u32, symbolize: &dyn Fn(u32) -> Option<String>) -> String {
    let insn = decode(word);
    if insn.op == Op::Illegal {
        return format!(".word 0x{word:08x}");
    }
    let (mnemonic, operands) = render(&insn, pc, symbolize);
    if operands.is_empty() {
        mnemonic.to_string()
    } else {
        format!("{mnemonic} {operands}")
    }
}

/// Signed hexadecimal as printed by llvm-objdump: `0x10`, `-0x4`.
fn hex(v: i32) -> String {
    if v < 0 {
        format!("-0x{:x}", -i64::from(v))
    } else {
        format!("0x{v:x}")
    }
}

fn csr_text(csr: u16) -> String {
    csr_name(csr).map_or_else(|| format!("0x{csr:x}"), str::to_string)
}

/// Text of a FENCE predecessor/successor set (bits `i o r w`), `"0"` if empty.
pub fn fence_set_name(bits: u32) -> String {
    let s: String = [(8, 'i'), (4, 'o'), (2, 'r'), (1, 'w')]
        .iter()
        .filter(|&&(bit, _)| bits & bit != 0)
        .map(|&(_, c)| c)
        .collect();
    if s.is_empty() {
        "0".to_string()
    } else {
        s
    }
}

fn render(i: &Insn, pc: u32, symbolize: &dyn Fn(u32) -> Option<String>) -> (&'static str, String) {
    use Op::*;
    let (rd, rs1, rs2) = (reg_name(i.rd), reg_name(i.rs1), reg_name(i.rs2));
    let target = || {
        let t = pc.wrapping_add(i.imm as u32);
        match symbolize(t) {
            Some(sym) => format!("0x{t:x} <{sym}>"),
            None => format!("0x{t:x}"),
        }
    };
    let mem = |r: &str| format!("{}({r})", hex(i.imm));
    let m = i.op.mnemonic();
    match i.op {
        Lui | Auipc => (m, format!("{rd}, 0x{:x}", (i.imm as u32) >> 12)),
        Jal => match i.rd {
            0 => ("j", target()),
            1 => ("jal", target()),
            _ => ("jal", format!("{rd}, {}", target())),
        },
        Jalr => match (i.rd, i.rs1, i.imm) {
            (0, 1, 0) => ("ret", String::new()),
            (0, _, 0) => ("jr", rs1.to_string()),
            (1, _, 0) => ("jalr", rs1.to_string()),
            (0, _, _) => ("jr", mem(rs1)),
            (1, _, _) => ("jalr", mem(rs1)),
            (_, _, 0) => ("jalr", format!("{rd}, {rs1}")),
            _ => ("jalr", format!("{rd}, {}", mem(rs1))),
        },
        Beq | Bne | Blt | Bge | Bltu | Bgeu => {
            let alias = match (i.op, i.rs1, i.rs2) {
                (Beq, _, 0) => Some(("beqz", rs1)),
                (Bne, _, 0) => Some(("bnez", rs1)),
                (Blt, _, 0) => Some(("bltz", rs1)),
                // `bge zero, zero` prints as `blez zero`, like llvm-objdump.
                (Bge, 0, _) => Some(("blez", rs2)),
                (Bge, _, 0) => Some(("bgez", rs1)),
                (Blt, 0, _) => Some(("bgtz", rs2)),
                _ => None,
            };
            match alias {
                Some((a, r)) => (a, format!("{r}, {}", target())),
                None => (m, format!("{rs1}, {rs2}, {}", target())),
            }
        }
        Lb | Lh | Lw | Lbu | Lhu => (m, format!("{rd}, {}", mem(rs1))),
        Sb | Sh | Sw => (m, format!("{rs2}, {}", mem(rs1))),
        Addi => match (i.rd, i.rs1, i.imm) {
            (0, 0, 0) => ("nop", String::new()),
            (_, 0, _) => ("li", format!("{rd}, {}", hex(i.imm))),
            (_, _, 0) => ("mv", format!("{rd}, {rs1}")),
            _ => (m, format!("{rd}, {rs1}, {}", hex(i.imm))),
        },
        Xori if i.imm == -1 => ("not", format!("{rd}, {rs1}")),
        Andi if i.imm == 255 => ("zext.b", format!("{rd}, {rs1}")),
        Sltiu if i.imm == 1 => ("seqz", format!("{rd}, {rs1}")),
        Slti | Sltiu | Xori | Ori | Andi => (m, format!("{rd}, {rs1}, {}", hex(i.imm))),
        Slli | Srli | Srai => (m, format!("{rd}, {rs1}, 0x{:x}", i.imm)),
        Sub if i.rs1 == 0 => ("neg", format!("{rd}, {rs2}")),
        Sltu if i.rs1 == 0 => ("snez", format!("{rd}, {rs2}")),
        Slt if i.rs2 == 0 => ("sltz", format!("{rd}, {rs1}")),
        Slt if i.rs1 == 0 => ("sgtz", format!("{rd}, {rs2}")),
        Add | Sub | Sll | Slt | Sltu | Xor | Srl | Sra | Or | And => {
            (m, format!("{rd}, {rs1}, {rs2}"))
        }
        Fence => {
            let fm = (i.imm >> 8) & 0xF;
            let pred = ((i.imm >> 4) & 0xF) as u32;
            let succ = (i.imm & 0xF) as u32;
            match (fm, pred, succ) {
                (0, 0xF, 0xF) => ("fence", String::new()),
                (8, 3, 3) => ("fence.tso", String::new()),
                (0, 1, 0) => ("pause", String::new()),
                _ => (
                    m,
                    format!("{}, {}", fence_set_name(pred), fence_set_name(succ)),
                ),
            }
        }
        FenceI | Ecall | Ebreak | Mret | Wfi => (m, String::new()),
        Csrrs if i.rs1 == 0 => {
            let counter = match i.csr {
                0xC00 => Some("rdcycle"),
                0xC80 => Some("rdcycleh"),
                0xC01 => Some("rdtime"),
                0xC81 => Some("rdtimeh"),
                0xC02 => Some("rdinstret"),
                0xC82 => Some("rdinstreth"),
                _ => None,
            };
            match counter {
                Some(c) => (c, rd.to_string()),
                None => ("csrr", format!("{rd}, {}", csr_text(i.csr))),
            }
        }
        Csrrw if i.rd == 0 && i.rs1 == 0 && i.csr == 0xC00 => ("unimp", String::new()),
        Csrrw | Csrrs | Csrrc if i.rd == 0 => {
            let a = match i.op {
                Csrrw => "csrw",
                Csrrs => "csrs",
                _ => "csrc",
            };
            (a, format!("{}, {rs1}", csr_text(i.csr)))
        }
        Csrrwi | Csrrsi | Csrrci if i.rd == 0 => {
            let a = match i.op {
                Csrrwi => "csrwi",
                Csrrsi => "csrsi",
                _ => "csrci",
            };
            (a, format!("{}, 0x{:x}", csr_text(i.csr), i.imm))
        }
        Csrrw | Csrrs | Csrrc => (m, format!("{rd}, {}, {rs1}", csr_text(i.csr))),
        Csrrwi | Csrrsi | Csrrci => (m, format!("{rd}, {}, 0x{:x}", csr_text(i.csr), i.imm)),
        Illegal => (m, String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(w: u32, pc: u32) -> String {
        disassemble(w, pc)
    }

    #[test]
    fn aliases_match_llvm_objdump() {
        let cases: &[(u32, u32, &str)] = &[
            (0x00000013, 0, "nop"),
            (0x00500513, 0, "li a0, 0x5"),
            (0x00058513, 0, "mv a0, a1"),
            (0xfff5c513, 0, "not a0, a1"),
            (0x40b00533, 0, "neg a0, a1"),
            (0x0015b513, 0, "seqz a0, a1"),
            (0x00b03533, 0, "snez a0, a1"),
            (0x0005a533, 0, "sltz a0, a1"),
            (0x00b02533, 0, "sgtz a0, a1"),
            (0x0ff5f513, 0, "zext.b a0, a1"),
            (0xfc050ae3, 0x2c, "beqz a0, 0x0"),
            (0xfca052e3, 0x3c, "blez a0, 0x0"),
            (0x00005063, 0, "blez zero, 0x0"),
            (0x00004063, 0, "bltz zero, 0x0"),
            (0x00002533, 0, "sltz a0, zero"),
            (0x00002073, 0, "csrr zero, 0x0"),
            (0xfca040e3, 0x40, "bgtz a0, 0x0"),
            (0xfaa5cee3, 0x44, "blt a1, a0, 0x0"),
            (0xfb9ff06f, 0x48, "j 0x0"),
            (0xfb5ff0ef, 0x4c, "jal 0x0"),
            (0xfb1ff56f, 0x50, "jal a0, 0x0"),
            (0x00008067, 0, "ret"),
            (0x00050067, 0, "jr a0"),
            (0x000500e7, 0, "jalr a0"),
            (0x00450067, 0, "jr 0x4(a0)"),
            (0x004500e7, 0, "jalr 0x4(a0)"),
            (0x004505e7, 0, "jalr a1, 0x4(a0)"),
            (0x000505e7, 0, "jalr a1, a0"),
            (0x30002573, 0, "csrr a0, mstatus"),
            (0x30051073, 0, "csrw mstatus, a0"),
            (0x3002d073, 0, "csrwi mstatus, 0x5"),
            (0x30001573, 0, "csrrw a0, mstatus, zero"),
            (0xc0002573, 0, "rdcycle a0"),
            (0xc8202573, 0, "rdinstreth a0"),
            (0xc0001073, 0, "unimp"),
            (0x7c002573, 0, "csrr a0, 0x7c0"),
            (0x0ff0000f, 0, "fence"),
            (0x0330000f, 0, "fence rw, rw"),
            (0x0100000f, 0, "pause"),
            (0x0000000f, 0, "fence 0, 0"),
            (0x8330000f, 0, "fence.tso"),
            (0x0000100f, 0, "fence.i"),
            (0xfffff537, 0, "lui a0, 0xfffff"),
            (0xffc12503, 0, "lw a0, -0x4(sp)"),
            (0x00a12423, 0, "sw a0, 0x8(sp)"),
            (0x01f51513, 0, "slli a0, a0, 0x1f"),
            (0x40155513, 0, "srai a0, a0, 0x1"),
            (0x10500073, 0, "wfi"),
            (0x00000000, 0, ".word 0x00000000"),
        ];
        for &(w, pc, text) in cases {
            assert_eq!(d(w, pc), text, "word {w:#010x}");
        }
    }

    #[test]
    fn symbolized_targets() {
        let sym = |a: u32| (a == 0x1000).then(|| "main".to_string());
        assert_eq!(
            disassemble_with(0x0000006f, 0x1000, &sym),
            "j 0x1000 <main>"
        );
        assert_eq!(disassemble_with(0x0000006f, 0x2000, &sym), "j 0x2000");
        // Targets wrap around the 32-bit address space.
        assert_eq!(d(0xff9ff06f, 0), "j 0xfffffff8");
    }
}
