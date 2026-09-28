//! Unit tests of instructions, pseudo-instruction expansions and relocations.

mod common;

use common::unit::*;
use strata_asm::LayoutSpec;
use strata_isa::encode::{hi20, lo12};
use strata_isa::{encode, Insn, Op};

fn e(op: Op, rd: u8, rs1: u8, rs2: u8, imm: i32) -> u32 {
    encode(&Insn {
        op,
        rd,
        rs1,
        rs2,
        imm,
        csr: 0,
    })
    .unwrap()
}

fn csr(op: Op, rd: u8, csr: u16, src: u8) -> u32 {
    let (rs1, imm) = if matches!(op, Op::Csrrwi | Op::Csrrsi | Op::Csrrci) {
        (0, i32::from(src))
    } else {
        (src, 0)
    };
    encode(&Insn {
        op,
        rd,
        rs1,
        rs2: 0,
        imm,
        csr,
    })
    .unwrap()
}

const RA: u8 = 1;
const T0: u8 = 5;
const T1: u8 = 6;
const A0: u8 = 10;
const A1: u8 = 11;

// ----- pseudo-instructions (docs/toolchain.md table) -------------------------------------------

#[test]
fn simple_pseudo_expansions() {
    use Op::*;
    let cases: Vec<(&str, Vec<u32>)> = vec![
        ("nop", vec![e(Addi, 0, 0, 0, 0)]),
        ("mv a0, a1", vec![e(Addi, A0, A1, 0, 0)]),
        ("not a0, a1", vec![e(Xori, A0, A1, 0, -1)]),
        ("neg a0, a1", vec![e(Sub, A0, 0, A1, 0)]),
        ("seqz a0, a1", vec![e(Sltiu, A0, A1, 0, 1)]),
        ("snez a0, a1", vec![e(Sltu, A0, 0, A1, 0)]),
        ("sltz a0, a1", vec![e(Slt, A0, A1, 0, 0)]),
        ("sgtz a0, a1", vec![e(Slt, A0, 0, A1, 0)]),
        ("zext.b a0, a1", vec![e(Andi, A0, A1, 0, 255)]),
        ("beqz a0, 8", vec![e(Beq, 0, A0, 0, 8)]),
        ("bnez a0, 8", vec![e(Bne, 0, A0, 0, 8)]),
        ("bltz a0, 8", vec![e(Blt, 0, A0, 0, 8)]),
        ("bgez a0, 8", vec![e(Bge, 0, A0, 0, 8)]),
        ("blez a0, 8", vec![e(Bge, 0, 0, A0, 8)]),
        ("bgtz a0, 8", vec![e(Blt, 0, 0, A0, 8)]),
        ("bgt a0, a1, 8", vec![e(Blt, 0, A1, A0, 8)]),
        ("ble a0, a1, 8", vec![e(Bge, 0, A1, A0, 8)]),
        ("bgtu a0, a1, 8", vec![e(Bltu, 0, A1, A0, 8)]),
        ("bleu a0, a1, 8", vec![e(Bgeu, 0, A1, A0, 8)]),
        ("j 16", vec![e(Jal, 0, 0, 0, 16)]),
        ("jal 16", vec![e(Jal, RA, 0, 0, 16)]),
        ("jr a0", vec![e(Jalr, 0, A0, 0, 0)]),
        ("ret", vec![e(Jalr, 0, RA, 0, 0)]),
        ("jalr a0", vec![e(Jalr, RA, A0, 0, 0)]),
        ("jalr a1, a0, -4", vec![e(Jalr, A1, A0, 0, -4)]),
        ("jalr a1, 12(a0)", vec![e(Jalr, A1, A0, 0, 12)]),
        ("csrr a0, mstatus", vec![csr(Csrrs, A0, 0x300, 0)]),
        ("csrw mtvec, a0", vec![csr(Csrrw, 0, 0x305, A0)]),
        ("csrs mie, a0", vec![csr(Csrrs, 0, 0x304, A0)]),
        ("csrc mip, a0", vec![csr(Csrrc, 0, 0x344, A0)]),
        ("csrwi mscratch, 31", vec![csr(Csrrwi, 0, 0x340, 31)]),
        ("csrsi mstatus, 8", vec![csr(Csrrsi, 0, 0x300, 8)]),
        ("csrci mstatus, 8", vec![csr(Csrrci, 0, 0x300, 8)]),
        ("rdcycle a0", vec![csr(Csrrs, A0, 0xC00, 0)]),
        ("rdcycleh a0", vec![csr(Csrrs, A0, 0xC80, 0)]),
        ("rdtime a0", vec![csr(Csrrs, A0, 0xC01, 0)]),
        ("rdtimeh a0", vec![csr(Csrrs, A0, 0xC81, 0)]),
        ("rdinstret a0", vec![csr(Csrrs, A0, 0xC02, 0)]),
        ("rdinstreth a0", vec![csr(Csrrs, A0, 0xC82, 0)]),
        ("unimp", vec![0xC000_1073]),
        ("fence", vec![0x0FF0_000F]),
        ("fence rw, w", vec![0x0310_000F]),
        ("fence.tso", vec![0x8330_000F]),
        ("fence.i", vec![0x0000_100F]),
        ("pause", vec![0x0100_000F]),
        ("ecall", vec![0x0000_0073]),
        ("ebreak", vec![0x0010_0073]),
        ("mret", vec![0x3020_0073]),
        ("wfi", vec![0x1050_0073]),
        // GNU/LLVM aliases.
        ("add a0, a1, 5", vec![e(Addi, A0, A1, 0, 5)]),
        ("sll a0, a1, 3", vec![e(Slli, A0, A1, 0, 3)]),
        ("csrw mstatus, 5", vec![csr(Csrrwi, 0, 0x300, 5)]),
        ("csrrs a0, 0x7c0, 3", vec![csr(Csrrsi, A0, 0x7C0, 3)]),
        (
            "la a0, 0x1234",
            vec![e(Addi, A0, 0, 0, 0x234), e(Addi, A0, A0, 0, 0x234)],
        ),
    ];
    for (src, expect) in cases {
        let got = text(src);
        if src.starts_with("la ") {
            // `la` with a constant is `li`.
            assert_eq!(got, text("li a0, 0x1234"), "{src}");
            continue;
        }
        assert_eq!(got, expect, "{src}");
    }
}

/// `li` per the spec: `addi` if the value fits in 12 bits, else `lui hi`
/// and `addi lo` (omitted when `lo == 0`), with `hi = (imm + 0x800) >> 12`.
#[test]
fn li_expansion() {
    use Op::*;
    let values: [i64; 16] = [
        0,
        1,
        -1,
        2047,
        -2048,
        2048,
        -2049,
        0x800,
        0x1000,
        0x7FFF_F800,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
        0xFFFF_F800,
        0x1234_5678,
        -0x8000_0000,
    ];
    for v in values {
        let v32 = v as u32 as i32;
        let expect = if (-2048..2048).contains(&v32) {
            vec![e(Addi, A0, 0, 0, v32)]
        } else {
            let hi = (i64::from(v32) + 0x800) >> 12;
            let lo = i64::from(v32) - (hi << 12);
            let mut w = vec![e(Lui, A0, 0, 0, ((hi as u32 & 0xFFFFF) << 12) as i32)];
            if lo != 0 {
                w.push(e(Addi, A0, A0, 0, lo as i32));
            }
            w
        };
        assert_eq!(text(&format!("li a0, {v}")), expect, "li {v:#x}");
    }
    assert_error("li a0, 0x100000000", "must fit in 32 bits");
    assert_error("li a0, sym", "known at this point");
}

#[test]
fn pc_relative_pseudo_expansions() {
    // Each pseudo equals its explicit %pcrel_hi/%pcrel_lo spelling.
    let pairs = [
        (
            "la a0, target",
            "1: auipc a0, %pcrel_hi(target)\n addi a0, a0, %pcrel_lo(1b)",
        ),
        (
            "lla a0, target",
            "1: auipc a0, %pcrel_hi(target)\n addi a0, a0, %pcrel_lo(1b)",
        ),
        (
            "call target",
            "1: auipc ra, %pcrel_hi(target)\n jalr ra, %pcrel_lo(1b)(ra)",
        ),
        (
            "call target@plt",
            "1: auipc ra, %pcrel_hi(target)\n jalr ra, %pcrel_lo(1b)(ra)",
        ),
        (
            "call t0, target",
            "1: auipc t0, %pcrel_hi(target)\n jalr t0, %pcrel_lo(1b)(t0)",
        ),
        (
            "tail target",
            "1: auipc t1, %pcrel_hi(target)\n jalr x0, %pcrel_lo(1b)(t1)",
        ),
        (
            "jump target, t2",
            "1: auipc t2, %pcrel_hi(target)\n jalr x0, %pcrel_lo(1b)(t2)",
        ),
        (
            "lb a0, target",
            "1: auipc a0, %pcrel_hi(target)\n lb a0, %pcrel_lo(1b)(a0)",
        ),
        (
            "lh a0, target",
            "1: auipc a0, %pcrel_hi(target)\n lh a0, %pcrel_lo(1b)(a0)",
        ),
        (
            "lw a0, target+8",
            "1: auipc a0, %pcrel_hi(target+8)\n lw a0, %pcrel_lo(1b)(a0)",
        ),
        (
            "lbu a0, target",
            "1: auipc a0, %pcrel_hi(target)\n lbu a0, %pcrel_lo(1b)(a0)",
        ),
        (
            "lhu a0, target",
            "1: auipc a0, %pcrel_hi(target)\n lhu a0, %pcrel_lo(1b)(a0)",
        ),
        (
            "sb a0, target, t0",
            "1: auipc t0, %pcrel_hi(target)\n sb a0, %pcrel_lo(1b)(t0)",
        ),
        (
            "sh a0, target, t0",
            "1: auipc t0, %pcrel_hi(target)\n sh a0, %pcrel_lo(1b)(t0)",
        ),
        (
            "sw a0, target, t0",
            "1: auipc t0, %pcrel_hi(target)\n sw a0, %pcrel_lo(1b)(t0)",
        ),
    ];
    // The target lies in another section, 0x1234 bytes beyond a 0x800 boundary,
    // so %pcrel_lo is negative.
    let tail = "\n.data\n.zero 0x1a34\ntarget: .word 0\n";
    for (pseudo, explicit) in pairs {
        let a = prog_at(&format!("nop\n{pseudo}{tail}"), 0x1000_0000);
        let b = prog_at(&format!("nop\n{explicit}{tail}"), 0x1000_0000);
        let (wa, wb) = (section_words(&a, ".text"), section_words(&b, ".text"));
        assert_eq!(wa, wb, "{pseudo}");
        assert_eq!(wa.len(), 3, "{pseudo}");
        // Check the pc-relative arithmetic itself.
        let target = sym(&a, "target");
        let pc = 0x1000_0004u32;
        let off = if pseudo.contains("+8") { 8 } else { 0 };
        let delta = i64::from((target + off).wrapping_sub(pc) as i32);
        assert_eq!(wa[1] >> 12, hi20(delta), "{pseudo}");
        let insn = strata_isa::decode(wa[2]);
        assert_eq!(insn.imm, lo12(delta), "{pseudo}");
    }
    let rd_of = |w: u32| (w >> 7) & 31;
    let w = text("call target\ntail target\njump target, t0\ntarget:");
    assert_eq!(
        [rd_of(w[0]), rd_of(w[1]), rd_of(w[2]), rd_of(w[4])],
        [u32::from(RA), 1, u32::from(T1), u32::from(T0)]
    );
    assert_error("call 0x100", "operand must be a symbol");
    assert_error("lw a0, 16", "operand must be a symbol");
}

// ----- relocation edge cases ---------------------------------------------------------------------

#[test]
fn hi_lo_rounding_with_negative_lo() {
    // x at 0x10000800: %hi rounds up and %lo is -2048.
    let p = prog_at(
        "lui a0, %hi(x)
         addi a1, a0, %lo(x)
         lw a2, %lo(x)(a0)
         sw a2, %lo(x+4)(a0)
         lui a3, %hi(y)
         addi a3, a3, %lo(y)
         .data
         .zero 0x800 - 24
         x: .word 1, 2
         .zero 0x7f8 - 8 + 0x7ff - 0x7ff
         y: .word 3",
        0x1000_0000,
    );
    let x = sym(&p, "x");
    assert_eq!(x, 0x1000_0800);
    let w = section_words(&p, ".text");
    assert_eq!(w[0], e(Op::Lui, A0, 0, 0, 0x1000_1000));
    assert_eq!(w[1], e(Op::Addi, A1, A0, 0, -2048));
    assert_eq!(w[2], e(Op::Lw, 12, A0, 0, -2048));
    assert_eq!(w[3], e(Op::Sw, 0, A0, 12, -2044));
    let y = i64::from(sym(&p, "y"));
    assert_eq!(w[4] >> 12, hi20(y));
    assert_eq!(strata_isa::decode(w[5]).imm, lo12(y));
    // Recombination is exact for any address.
    for v in [0x7FFi64, 0x800, 0xFFF, 0x1000, 0xFFFF_F800, 0x8000_0000] {
        assert_eq!((hi20(v) << 12).wrapping_add(lo12(v) as u32), v as u32);
    }
}

#[test]
fn pcrel_lo_pairing() {
    let w = text(
        "nop
         .Lhi: auipc a0, %pcrel_hi(data)
         nop
         addi a0, a0, %pcrel_lo(.Lhi)
         2: auipc a1, %pcrel_hi(data + 4)
         lw a1, %pcrel_lo(2b)(a1)
         sw a1, %pcrel_lo(2b)(a1)
         data: .word 0",
    );
    // data is at 28; auipc at 4 -> delta 24; auipc at 16 -> delta 16.
    assert_eq!(strata_isa::decode(w[3]).imm, 24);
    assert_eq!(strata_isa::decode(w[5]).imm, 16);
    assert_eq!(strata_isa::decode(w[6]).imm, 16);
    assert_error(
        "nop\n.Lx: nop\naddi a0, a0, %pcrel_lo(.Lx)",
        "does not point at an auipc with a %pcrel_hi",
    );
    assert_error(
        ".Ly: auipc a0, %hi(5)\naddi a0, a0, %pcrel_lo(.Ly)",
        "operand must be",
    );
    assert_error(
        "addi a0, a0, %pcrel_lo(5)",
        "%pcrel_lo requires the label of an auipc",
    );
}

#[test]
fn branch_and_jump_ranges() {
    // Exactly at the limits.
    let w = text("beq a0, a1, fwd\n.zero 4090\nfwd: nop");
    assert_eq!(strata_isa::decode(w[0]).imm, 4094);
    let w = text("back: nop\n.zero 4092\nbne a0, a1, back");
    assert_eq!(strata_isa::decode(*w.last().unwrap()).imm, -4096);
    assert_error(
        "beq a0, a1, fwd\n.zero 4092\nfwd: nop",
        "branch target 0x1000 out of range",
    );
    assert_error("back: nop\n.zero 4096\nbeq a0, a1, back", "out of range");
    assert_error("beq a0, a1, odd\n.byte 0\nodd: nop", "not 2-byte aligned");
    let w = text("jal fwd\n.zero 1048570\nfwd: nop");
    assert_eq!(strata_isa::decode(w[0]).imm, 1_048_574);
    assert_error(
        "jal fwd\n.zero 1048572\nfwd: nop",
        "jump target 0x100000 out of range",
    );
    // Numeric operands are offsets and are checked immediately.
    assert_error(
        "beq a0, a1, 4096",
        "multiple of 2 bytes in the range [-4096, 4094]",
    );
    assert_error(
        "j 1048576",
        "multiple of 2 bytes in the range [-1048576, 1048574]",
    );
    // Link-time errors point at the instruction.
    let d = diags("nop\n  beq a0, a1, far\n.zero 8000\nfar:");
    assert!(d.contains("t.s:2:3: error: branch target"), "{d}");
}

#[test]
fn far_calls_wrap_the_address_space() {
    use strata_asm::layout::parse_layout;
    let mut dg = strata_asm::diag::Diags::new();
    let l = parse_layout(
        "memory LO origin=0 length=4K\nmemory HI origin=0xF0000000 length=4K\noutput .text in LO { .text }\noutput .far in HI { .far }\n",
        strata_asm::diag::FileId(0),
        &mut dg,
    )
    .unwrap();
    let p = progs(
        &[(
            "t.s",
            "call far\nj near\nnear: nop\n.section .far, \"ax\"\nfar: ret",
        )],
        LayoutSpec::Parsed(l),
    );
    let w = section_words(&p, ".text");
    // 0xF0000000 - 0 = -0x10000000 as a 32-bit displacement.
    assert_eq!(w[0] >> 12, 0xF0000);
    assert_eq!(strata_isa::decode(w[1]).imm, 0);
}

#[test]
fn data_relocations() {
    let p = prog_at(
        ".data
         a: .word b, b + 4, b - a, c - a
         .half b - a
         .byte 2f - 1f
         1: .zero 3
         2:
         b: .word 0
         .section .rodata.x, \"a\"
         c: .word a",
        0x2000,
    );
    let (a, b, c) = (sym(&p, "a"), sym(&p, "b"), sym(&p, "c"));
    let d = section_bytes(&p, ".data");
    let word = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    assert_eq!(
        [word(0), word(4), word(8), word(12)],
        [b, b + 4, b - a, c.wrapping_sub(a)]
    );
    assert_eq!(u16::from_le_bytes([d[16], d[17]]) as u32, b - a);
    assert_eq!(d[18], 3);
    assert_eq!(section_words(&p, ".rodata"), vec![a]);
    assert_error(".data\nx: .half y\ny:", "relocatable value in 2-byte data");
    assert_error(
        ".data\n.half later - here\nhere: .zero 70000\nlater:",
        "does not fit in 2 byte",
    );
    assert_error(
        ".data\n.word undefined_sym",
        "undefined symbol 'undefined_sym'",
    );
}

#[test]
fn deferred_immediates() {
    let w = text(
        "addi a0, a0, LATER
         sw a0, LATER(a1)
         slli a0, a0, SH
         lui a0, UP
         addi a0, a0, end - start
         start: nop
         end:
         .equ LATER, -5
         .equ SH, 31
         .equ UP, 0xABCDE",
    );
    assert_eq!(w[0], e(Op::Addi, A0, A0, 0, -5));
    assert_eq!(w[1], e(Op::Sw, 0, A1, A0, -5));
    assert_eq!(w[2], e(Op::Slli, A0, A0, 0, 31));
    assert_eq!(w[3], e(Op::Lui, A0, 0, 0, (0xABCDEu32 << 12) as i32));
    assert_eq!(w[4], e(Op::Addi, A0, A0, 0, 4));
    assert_error(
        "addi a0, a0, BIG\n.equ BIG, 4096",
        "immediate 4096 out of range",
    );
    assert_error("slli a0, a0, S\n.equ S, 32", "shift amount 32 out of range");
    assert_error("lui a0, U\n.equ U, -1", "out of range [0, 1048575]");
    assert_error(
        "lbl: addi a0, a0, lbl",
        "operand must be a symbol with a %lo",
    );
    assert_error(
        "addi a0, a0, fwd\nnop\nfwd:",
        "immediate must be an absolute value",
    );
}

#[test]
fn weak_and_undefined_symbols() {
    let w = text(".weak maybe\ncall maybe\n.word maybe");
    // Undefined weak symbols resolve to 0: auipc/jalr reach address 0 from 0.
    assert_eq!(
        w,
        vec![e(Op::Auipc, RA, 0, 0, 0), e(Op::Jalr, RA, RA, 0, 0), 0]
    );
    let d = diags("nop\n  call missing_fn\n");
    assert!(
        d.contains("t.s:2:3: error: undefined symbol 'missing_fn'"),
        "{d}"
    );
}

#[test]
fn section_names_as_symbols() {
    let p = prog_at(".data\n.word 1\n.word .data\n.text\nnop", 0x100);
    assert_eq!(
        section_words(&p, ".data")[1],
        p.sections.iter().find(|s| s.name == ".data").unwrap().addr
    );
}

#[test]
fn operand_errors() {
    assert_error("add a0, a1", "too few operands");
    assert_error("add a0, a1, a2, a3", "too many operands");
    assert_error("addi a0, a1, a2", "found register");
    assert_error("lw a0, 4(a9)", "expected a register");
    assert_error("lw a0, %lo(x)(a1", "relocation operator not allowed here");
    assert_error("fence rw, x", "iorw");
    assert_error("fence wr, r", "iorw");
    assert_error("csrrw a0, nosuch, a1", "valid CSR name");
    assert_error("csrrw a0, 4096, a1", "valid CSR name");
    assert_error("csrwi mstatus, 32", "range [0, 31]");
    assert_error("lui a0, %lo(4)", "%hi modifier");
    assert_error("auipc a0, %hi(4)", "%pcrel_hi modifier");
    assert_error("addi a0, a0, %hi(4)", "%lo/%pcrel_lo");
    assert_error("addi a0, a0, %bogus(4)", "unsupported relocation operator");
    assert_error("sret", "unknown instruction 'sret'");
    assert_error("mul a0, a1, a2", "unknown instruction 'mul'");
    assert_error("nop a0", "too many operands");
    assert_error("addi a0,, a1", "expected an operand");
}

#[test]
fn register_and_csr_spellings() {
    let w = text("add x10, fp, s0\nadd A0, X8, S0\ncsrr a0, 0x300\ncsrr a0, MSTATUS\ncsrr a0, 768\ncsrr a0, pmpaddr15\ncsrr a0, mhpmcounter31h");
    assert_eq!(w[0], w[1]);
    assert_eq!(w[2], w[3]);
    assert_eq!(w[3], w[4]);
    assert_eq!(w[5] >> 20, 0x3BF);
    assert_eq!(w[6] >> 20, 0xB9F);
}

#[test]
fn mnemonics_are_case_insensitive() {
    assert_eq!(
        text("ADDI a0, a0, 1\nNop\nFENCE.I"),
        text("addi a0, a0, 1\nnop\nfence.i")
    );
}
