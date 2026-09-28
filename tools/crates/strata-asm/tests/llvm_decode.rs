//! Differential test (c): `strata_isa::decode` and `disassemble` versus
//! `llvm-objdump --mattr=+zicsr,+zifencei`.
//!
//! About 300k words (random, random per major opcode, and exhaustive sweeps
//! of the funct fields) are written into `.text` of an ELF built with
//! strata-as and disassembled by llvm-objdump. For every word, "illegal in
//! Strata" must coincide with llvm-objdump's `<unknown>`, and for words both
//! decode the disassembly text must be identical, except for these
//! intentional differences (`docs/isa.md`):
//!
//! * `FENCE` ignores `rd`, `rs1` and `fm` and `FENCE.I` ignores `rd`, `rs1`
//!   and `imm`; LLVM only decodes them with those fields zero (and `fm` = 0,
//!   or the exact `fence.tso` encoding).
//! * `SRET`, `DRET` and `SFENCE.VMA` are illegal on Strata (no S-mode, no
//!   debug mode); LLVM decodes them unconditionally.
//! * Strata names only its own CSRs; LLVM also names CSRs Strata lacks
//!   (e.g. `sstatus`), which Strata prints as numbers.
//! * llvm-objdump prints two hint encodings with aliases of extensions that
//!   are not enabled: `auipc zero, imm` as `lpad imm` (Zicfilp) and
//!   `ori zero, rs, imm` as `prefetch.i/r/w` (Zicbop). Strata shows them as
//!   the base instructions they are.
//!
//! Words whose low bits are not `11` or whose bits `[4:2]` are `111` encode
//! 16-bit or 48/64-bit instructions; llvm-objdump would consume 2 or 6+
//! bytes for them, desynchronizing the stream, so they are checked
//! separately (they are always illegal on Strata).

mod common;

use common::{
    normalize_objdump as normalize, objdump_text_matches as texts_match, run, scratch, strata,
    tools, Rng,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::Command;
use strata_asm::LayoutSpec;
use strata_isa::{decode, disassemble, Op};

/// Why Strata intentionally differs from LLVM for `w`, if it does.
fn documented_difference(w: u32) -> Option<&'static str> {
    let opcode = w & 0x7F;
    let funct3 = (w >> 12) & 7;
    let (rd, rs1) = ((w >> 7) & 31, (w >> 15) & 31);
    if opcode == 0x0F && funct3 == 0 {
        let fm = w >> 28;
        let pred_succ = (w >> 20) & 0xFF;
        let llvm_ok = rd == 0 && rs1 == 0 && (fm == 0 || (fm == 8 && pred_succ == 0x33));
        return (!llvm_ok).then_some("FENCE ignores rd/rs1/fm");
    }
    if opcode == 0x0F && funct3 == 1 && (w >> 7) != 0x20 {
        return Some("FENCE.I ignores rd/rs1/imm");
    }
    if w == 0x1020_0073 || w == 0x7B20_0073 {
        return Some("SRET/DRET are not implemented");
    }
    if opcode == 0x73 && funct3 == 0 && rd == 0 && (w >> 25) == 0b000_1001 {
        return Some("SFENCE.VMA is not implemented");
    }
    None
}

fn gen_words() -> Vec<u32> {
    let mut rng = Rng::new(0xDEC0_DE00_0001);
    let mut words = Vec::new();
    // Random words with 32-bit length encoding, plus some fully random ones.
    for _ in 0..150_000 {
        words.push(rng.next_u32() | 3);
    }
    for _ in 0..20_000 {
        words.push(rng.next_u32());
    }
    // Random operands for every major opcode (legal and illegal ones).
    let opcodes = [
        0x03, 0x07, 0x0F, 0x13, 0x17, 0x1B, 0x23, 0x27, 0x2F, 0x33, 0x37, 0x3B, 0x43, 0x47, 0x4B,
        0x4F, 0x53, 0x57, 0x5B, 0x63, 0x67, 0x6B, 0x6F, 0x73, 0x77, 0x7B,
    ];
    for &op in &opcodes {
        let n = if [
            0x03, 0x0F, 0x13, 0x17, 0x23, 0x33, 0x37, 0x63, 0x67, 0x6F, 0x73,
        ]
        .contains(&op)
        {
            8000
        } else {
            1500
        };
        for _ in 0..n {
            words.push(rng.next_u32() & !0x7F | op);
        }
    }
    // Exhaustive funct7 x funct3 for OP, and funct7 for the shift immediates.
    for f7 in 0..128u32 {
        for f3 in 0..8u32 {
            let regs = rng.next_u32() & 0x01FF_8F80;
            words.push(f7 << 25 | f3 << 12 | regs | 0x33);
        }
        for f3 in [1u32, 5] {
            let regs = rng.next_u32() & 0x01FF_8F80;
            words.push(f7 << 25 | f3 << 12 | regs | 0x13);
        }
    }
    // Every SYSTEM funct3=0 immediate with zero registers, plus variants with
    // one nonzero register field.
    for imm in 0..4096u32 {
        words.push(imm << 20 | 0x73);
        words.push(imm << 20 | (rng.next_u32() & 31) << 7 | 0x73);
        words.push(imm << 20 | (rng.next_u32() & 31) << 15 | 0x73);
    }
    // Every FENCE immediate with zero registers.
    for imm in 0..4096u32 {
        words.push(imm << 20 | 0x0F);
    }
    words.push(0);
    words.push(u32::MAX);
    words
}

/// Words whose instruction length (from the low bits) is not 32 bits.
fn not_32bit(w: u32) -> bool {
    w & 3 != 3 || w & 0x1C == 0x1C
}

#[test]
fn decoder_matches_llvm_objdump() {
    let Some(t) = tools("decoder_matches_llvm_objdump", &["llvm-objdump"]) else {
        return;
    };
    let dir = scratch("llvm_decode");
    let all = gen_words();
    let (odd, words): (Vec<u32>, Vec<u32>) = all.iter().partition(|&&w| not_32bit(w));
    for w in &odd {
        assert_eq!(
            decode(*w).op,
            Op::Illegal,
            "{w:#010x} is not a 32-bit encoding"
        );
    }

    let mut src = String::from("\t.text\n");
    for w in &words {
        let _ = writeln!(src, "\t.word 0x{w:08x}");
    }
    let (p, _) = strata(&[("words.s", &src)], LayoutSpec::Org(0));
    let elf = dir.join("words.elf");
    std::fs::write(&elf, strata_asm::output::write_elf(&p)).unwrap();
    let out = run(Command::new(&t[0])
        .args(["-d", "--mattr=+zicsr,+zifencei"])
        .arg(&elf));

    // address -> (raw hex, text)
    let mut llvm: BTreeMap<u32, (String, String)> = BTreeMap::new();
    for line in out.lines() {
        let Some((left, text)) = line.split_once('\t') else {
            continue;
        };
        let Some((addr, raw)) = left.split_once(':') else {
            continue;
        };
        let Ok(addr) = u32::from_str_radix(addr.trim(), 16) else {
            continue;
        };
        llvm.insert(addr, (raw.trim().to_string(), text.trim().to_string()));
    }

    let mut differences: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failures = String::new();
    let mut nfail = 0;
    let (mut legal, mut compared_text) = (0, 0);
    for (i, &w) in words.iter().enumerate() {
        let addr = (i * 4) as u32;
        let Some((raw, text)) = llvm.get(&addr) else {
            panic!("llvm-objdump output has no entry at {addr:#x} (word {w:#010x}); stream out of sync");
        };
        assert_eq!(
            raw,
            &format!("{w:08x}"),
            "llvm-objdump decoded a different length at {addr:#x}"
        );
        let insn = decode(w);
        let ours_illegal = insn.op == Op::Illegal;
        let llvm_illegal = text.starts_with("<unknown>");
        if let Some(reason) = documented_difference(w) {
            // Only the documented direction is allowed.
            let expected_ours_illegal = reason.contains("not implemented");
            assert_eq!(ours_illegal, expected_ours_illegal, "{w:#010x}: {reason}");
            if ours_illegal == llvm_illegal {
                // LLVM agrees after all; nothing to excuse.
            } else {
                *differences.entry(reason).or_default() += 1;
            }
            continue;
        }
        if ours_illegal != llvm_illegal {
            nfail += 1;
            if nfail <= 40 {
                let _ = writeln!(failures, "  {w:#010x}: strata {:?} llvm {text:?}", insn.op);
            }
            continue;
        }
        if ours_illegal {
            continue;
        }
        legal += 1;
        let ours = normalize(&disassemble(w, addr));
        let theirs = normalize(text);
        compared_text += 1;
        if !texts_match(&ours, &theirs, &insn) {
            nfail += 1;
            if nfail <= 40 {
                let _ = writeln!(failures, "  {w:#010x}: strata {ours:?} llvm {theirs:?}");
            }
        }
    }
    eprintln!(
        "decode corpus: {} words ({} 32-bit, {} other lengths); {legal} legal in both, {compared_text} texts compared; documented differences: {differences:?}",
        all.len(),
        words.len(),
        odd.len()
    );
    assert!(nfail == 0, "{nfail} mismatches:\n{failures}");
}
