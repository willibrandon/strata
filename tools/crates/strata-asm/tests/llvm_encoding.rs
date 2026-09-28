//! Differential test (a): instruction encodings of strata-as versus
//! `llvm-mc` (+ `ld.lld` to resolve the few relocations llvm-mc leaves).
//!
//! A generated corpus covers every instruction and pseudo-instruction form
//! with boundary and random operands, register spellings, label and numeric
//! targets, and `%hi`/`%lo`/`%pcrel_hi`/`%pcrel_lo` pairs. Both assemblers
//! must produce identical `.text` words. A second corpus of invalid
//! operands must be rejected line-for-line by both.

mod common;

use common::{
    llvm_text_bytes, normalize_objdump, objdump_text_matches, run, scratch, strata, tools, words,
    Rng, MC_ARGS,
};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::process::Command;
use strata_asm::LayoutSpec;

const ABI: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
    "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
    "t5", "t6",
];

fn reg(rng: &mut Rng) -> String {
    let r = rng.range(0, 31) as usize;
    match rng.range(0, 5) {
        0 => format!("x{r}"),
        1 if r == 8 => "fp".to_string(),
        _ => ABI[r].to_string(),
    }
}

/// Spells an integer in one of several syntaxes both assemblers accept.
fn spell(rng: &mut Rng, v: i64) -> String {
    match rng.range(0, 5) {
        0 if v >= 0 => format!("0x{v:x}"),
        1 if v < 0 => format!("-0x{:x}", -v),
        2 if (32..127).contains(&v) && v != 39 && v != 92 => format!("'{}'", v as u8 as char),
        3 => format!("({} + {})", v - 3, 3),
        _ => v.to_string(),
    }
}

fn imm12(rng: &mut Rng) -> i64 {
    const B: [i64; 13] = [
        -2048, -2047, -1024, -1, 0, 1, 2, 7, 255, 256, 1023, 2046, 2047,
    ];
    if rng.range(0, 2) == 0 {
        *rng.pick(&B)
    } else {
        rng.range(-2048, 2047)
    }
}

/// Generates the valid corpus. Labels `.LbN` are placed every 16 lines so
/// label targets stay within branch range.
fn corpus(rng: &mut Rng) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut block = 0usize;
    let push = |lines: &mut Vec<String>, block: &mut usize, s: String| {
        if lines.len() % 16 == 0 {
            lines.push(format!(".Lb{}:", *block));
            *block += 1;
        }
        lines.push(s);
    };
    // A label near the current position (previous, current or next block).
    let near = |rng: &mut Rng, block: usize| -> String {
        let k = (block as i64 + rng.range(-1, 1)).max(0);
        format!(".Lb{k}")
    };

    let r_ops = [
        "add", "sub", "sll", "slt", "sltu", "xor", "srl", "sra", "or", "and",
    ];
    for op in r_ops {
        for rd in ABI {
            let s = format!("{op} {rd}, {}, {}", reg(rng), reg(rng));
            push(&mut lines, &mut block, s);
        }
    }
    for op in ["addi", "slti", "sltiu", "xori", "ori", "andi"] {
        for _ in 0..60 {
            let v = imm12(rng);
            let s = format!("{op} {}, {}, {}", reg(rng), reg(rng), spell(rng, v));
            push(&mut lines, &mut block, s);
        }
    }
    for op in ["slli", "srli", "srai"] {
        for sh in 0..32 {
            let s = format!("{op} {}, {}, {sh}", reg(rng), reg(rng));
            push(&mut lines, &mut block, s);
        }
    }
    for op in ["lb", "lh", "lw", "lbu", "lhu", "sb", "sh", "sw"] {
        for i in 0..40 {
            let v = imm12(rng);
            let big = rng.range(-100_000, 100_000);
            let mem = match i % 4 {
                0 => format!("({})", reg(rng)),
                1 => format!("%lo({})({})", spell(rng, big), reg(rng)),
                _ => format!("{}({})", spell(rng, v), reg(rng)),
            };
            let s = format!("{op} {}, {mem}", reg(rng));
            push(&mut lines, &mut block, s);
        }
    }
    for op in [
        "beq", "bne", "blt", "bge", "bltu", "bgeu", "bgt", "ble", "bgtu", "bleu",
    ] {
        for off in [-4096, -4094, -2, 0, 2, 4094] {
            let s = format!("{op} {}, {}, {off}", reg(rng), reg(rng));
            push(&mut lines, &mut block, s);
        }
        for _ in 0..20 {
            let off = rng.range(-2048, 2047) * 2;
            let s = format!("{op} {}, {}, {off}", reg(rng), reg(rng));
            push(&mut lines, &mut block, s);
            let t = near(rng, block);
            let s = format!("{op} {}, {}, {t}", reg(rng), reg(rng));
            push(&mut lines, &mut block, s);
        }
    }
    for op in ["beqz", "bnez", "bltz", "bgez", "blez", "bgtz"] {
        for _ in 0..12 {
            let t = if rng.range(0, 1) == 0 {
                near(rng, block)
            } else {
                (rng.range(-2048, 2047) * 2).to_string()
            };
            let s = format!("{op} {}, {t}", reg(rng));
            push(&mut lines, &mut block, s);
        }
    }
    for v in [0i64, 1, 0x7ffff, 0x80000, 0xfffff, 0x12345] {
        for op in ["lui", "auipc"] {
            let s = format!("{op} {}, {}", reg(rng), spell(rng, v));
            push(&mut lines, &mut block, s);
        }
    }
    for _ in 0..30 {
        let v = rng.range(0, 0xfffff);
        let s = format!("lui {}, {}", reg(rng), spell(rng, v));
        push(&mut lines, &mut block, s);
        let c = rng.range(-(1 << 31), (1 << 31) - 1);
        let s = format!("lui {}, %hi({c})", reg(rng));
        push(&mut lines, &mut block, s);
        let s = format!("addi {}, {}, %lo({c})", reg(rng), reg(rng));
        push(&mut lines, &mut block, s);
        let t = near(rng, block);
        let s = format!("auipc {}, %pcrel_hi({t})", reg(rng));
        push(&mut lines, &mut block, s);
    }
    for off in [-(1i64 << 20), -(1 << 20) + 2, -2, 0, 2, (1 << 20) - 2] {
        let s = format!("jal {}, {off}", reg(rng));
        push(&mut lines, &mut block, s);
        push(&mut lines, &mut block, format!("j {off}"));
        push(&mut lines, &mut block, format!("jal {off}"));
    }
    for _ in 0..30 {
        let t = near(rng, block);
        let s = match rng.range(0, 3) {
            0 => format!("jal {}, {t}", reg(rng)),
            1 => format!("j {t}"),
            2 => format!("jal {t}"),
            _ => format!(
                "jal {}, {}",
                reg(rng),
                rng.range(-(1 << 19), (1 << 19) - 1) * 2
            ),
        };
        push(&mut lines, &mut block, s);
    }
    for _ in 0..30 {
        let v = imm12(rng);
        let s = match rng.range(0, 7) {
            0 => format!("jalr {}", reg(rng)),
            1 => format!("jalr {}, {}", reg(rng), reg(rng)),
            2 => format!("jalr {}, {}({})", reg(rng), spell(rng, v), reg(rng)),
            3 => format!("jalr {}, {}, {}", reg(rng), reg(rng), spell(rng, v)),
            4 => format!("jalr {}({})", spell(rng, v), reg(rng)),
            5 => format!("jr {}", reg(rng)),
            6 => format!("jr {}({})", spell(rng, v), reg(rng)),
            _ => "ret".to_string(),
        };
        push(&mut lines, &mut block, s);
    }
    // CSRs: every implemented name, and random numbers.
    for addr in strata_isa::all_csrs() {
        let name = strata_isa::csr_name(addr).unwrap_or_default();
        let s = format!("csrrs {}, {name}, {}", reg(rng), reg(rng));
        push(&mut lines, &mut block, s);
    }
    for op in ["csrrw", "csrrs", "csrrc"] {
        for _ in 0..15 {
            let s = format!("{op} {}, {}, {}", reg(rng), rng.range(0, 4095), reg(rng));
            push(&mut lines, &mut block, s);
        }
    }
    for op in ["csrrwi", "csrrsi", "csrrci"] {
        for _ in 0..15 {
            let s = format!(
                "{op} {}, {}, {}",
                reg(rng),
                rng.range(0, 4095),
                rng.range(0, 31)
            );
            push(&mut lines, &mut block, s);
        }
    }
    let csr_names = [
        "mstatus", "mtvec", "mepc", "mcause", "mscratch", "mie", "mip", "cycle", "0x7c0", "0xb00",
    ];
    for _ in 0..40 {
        let c = rng.pick(&csr_names).to_string();
        let s = match rng.range(0, 6) {
            0 => format!("csrr {}, {c}", reg(rng)),
            1 => format!("csrw {c}, {}", reg(rng)),
            2 => format!("csrs {c}, {}", reg(rng)),
            3 => format!("csrc {c}, {}", reg(rng)),
            4 => format!("csrwi {c}, {}", rng.range(0, 31)),
            5 => format!("csrsi {c}, {}", rng.range(0, 31)),
            _ => format!("csrci {c}, {}", rng.range(0, 31)),
        };
        push(&mut lines, &mut block, s);
    }
    for op in [
        "rdcycle",
        "rdcycleh",
        "rdtime",
        "rdtimeh",
        "rdinstret",
        "rdinstreth",
    ] {
        let s = format!("{op} {}", reg(rng));
        push(&mut lines, &mut block, s);
    }
    let set = |bits: u32| -> String {
        let s: String = [(8, 'i'), (4, 'o'), (2, 'r'), (1, 'w')]
            .iter()
            .filter(|(b, _)| bits & b != 0)
            .map(|(_, c)| *c)
            .collect();
        if s.is_empty() {
            "0".into()
        } else {
            s
        }
    };
    for pred in 0..16 {
        for succ in 0..16 {
            push(
                &mut lines,
                &mut block,
                format!("fence {}, {}", set(pred), set(succ)),
            );
        }
    }
    for s in [
        "fence",
        "fence.tso",
        "fence.i",
        "pause",
        "ecall",
        "ebreak",
        "mret",
        "wfi",
        "unimp",
        "nop",
        "ret",
    ] {
        push(&mut lines, &mut block, s.to_string());
    }
    // li across the interesting boundaries and random 32-bit values.
    let li_vals: Vec<i64> = vec![
        0,
        1,
        -1,
        2047,
        2048,
        -2048,
        -2049,
        0x7ff,
        0x800,
        0xfff,
        0x1000,
        0x1001,
        0x7fff_f7ff,
        0x7fff_f800,
        0x7fff_ffff,
        -0x8000_0000,
        0x8000_0000,
        0xffff_ffff,
        0xffff_f800,
        0x1234_5678,
        0x0001_0000,
        0x4000_1000,
    ];
    let mut li_vals = li_vals;
    for _ in 0..80 {
        li_vals.push(if rng.range(0, 1) == 0 {
            rng.range(-(1 << 31), (1 << 31) - 1)
        } else {
            rng.range(0, (1 << 32) - 1)
        });
    }
    for v in li_vals {
        // `li` is marked with its value so the LLVM side can use the
        // expansion from the spec (see `llvm_source`).
        let s = format!("li {}, {} {LI_MARK}{v}", reg(rng), spell(rng, v));
        push(&mut lines, &mut block, s);
    }
    for op in ["mv", "not", "neg", "seqz", "snez", "sltz", "sgtz", "zext.b"] {
        for _ in 0..6 {
            let s = format!("{op} {}, {}", reg(rng), reg(rng));
            push(&mut lines, &mut block, s);
        }
    }
    // PC-relative pseudo-instructions to labels.
    for i in 0..25 {
        let t = near(rng, block);
        let rt = reg(rng);
        let s = match i % 12 {
            0 => format!("la {}, {t}", reg(rng)),
            1 => format!("lla {}, {t}", reg(rng)),
            2 => format!("call {t}"),
            3 => format!("tail {t}"),
            4 => format!("jump {t}, {rt}"),
            5 => format!("lw {}, {t}", reg(rng)),
            6 => format!("lbu {}, {t}+3", reg(rng)),
            7 => format!("sw {}, {t}, {rt}", reg(rng)),
            8 => format!("sh {}, {t}-2, {rt}", reg(rng)),
            9 => format!("call {t}@plt"),
            10 => format!("call {}, {t}", reg(rng)),
            _ => format!("lui {}, %hi({t})", reg(rng)),
        };
        push(&mut lines, &mut block, s);
    }
    // Explicit %pcrel_hi/%pcrel_lo pairs, with named and numeric labels.
    for i in 0..10 {
        let t = near(rng, block);
        let (r1, r2) = (reg(rng), reg(rng));
        let lbl = if i % 2 == 0 {
            format!(".Lpc{i}")
        } else {
            "1".to_string()
        };
        let reflbl = if i % 2 == 0 {
            lbl.clone()
        } else {
            "1b".to_string()
        };
        push(
            &mut lines,
            &mut block,
            format!("{lbl}: auipc {r1}, %pcrel_hi({t}+{})", 4 * i),
        );
        push(
            &mut lines,
            &mut block,
            format!("addi {r2}, {r1}, %pcrel_lo({reflbl})"),
        );
        push(
            &mut lines,
            &mut block,
            format!("lw {r2}, %pcrel_lo({reflbl})({r1})"),
        );
        push(
            &mut lines,
            &mut block,
            format!("sw {r2}, %pcrel_lo({reflbl})({r1})"),
        );
    }
    // %hi/%lo of labels.
    for _ in 0..10 {
        let t = near(rng, block);
        push(&mut lines, &mut block, format!("lui a0, %hi({t})"));
        push(&mut lines, &mut block, format!("addi a0, a0, %lo({t})"));
        push(&mut lines, &mut block, format!("sw a1, %lo({t}+8)(a0)"));
    }
    // Numeric local labels, backward and forward.
    for _ in 0..8 {
        push(&mut lines, &mut block, "2: nop".to_string());
        push(&mut lines, &mut block, "beqz a0, 2b".to_string());
        push(&mut lines, &mut block, "bnez a0, 3f".to_string());
        push(&mut lines, &mut block, "j 2b".to_string());
        push(&mut lines, &mut block, "3: addi a0, a0, 1".to_string());
    }
    // Targets of `near` that lie beyond the last block.
    lines.push(format!(".Lb{block}:"));
    lines.push(format!(".Lb{}:", block + 1));
    lines
}

/// Marks `li` lines in the corpus; followed by the value.
const LI_MARK: &str = "# li=";

/// The spec's `li` expansion (`docs/toolchain.md`), computed independently
/// of the assembler: `addi` if the value fits in 12 bits, else `lui hi` and
/// `addi lo` (omitted when `lo == 0`), `hi = (imm + 0x800) >> 12`.
fn spec_li(rd: &str, v: i64) -> String {
    let v = i64::from(v as u32 as i32);
    if (-2048..2048).contains(&v) {
        return format!("addi {rd}, zero, {v}");
    }
    let hi = (v + 0x800) >> 12;
    let lo = v - (hi << 12);
    let lui = format!("lui {rd}, {}", hi & 0xFFFFF);
    if lo == 0 {
        lui
    } else {
        format!("{lui}\naddi {rd}, {rd}, {lo}")
    }
}

/// Source for llvm-mc: identical to the corpus except that `li` lines use
/// the spec's expansion. LLVM's own `li` picks sequences with its constant
/// materializer (e.g. `li a0, 2048` becomes `addi a0, zero, 1; slli a0,
/// a0, 11`), whereas the spec defines the exact `lui`/`addi` expansion.
fn llvm_source(lines: &[String]) -> String {
    let mut out = String::from("\t.text\n");
    for l in lines {
        match l.split_once(LI_MARK) {
            Some((insn, v)) => {
                let rd = insn
                    .trim_start_matches("li ")
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim();
                out.push_str(&spec_li(rd, v.trim().parse().unwrap_or(0)));
            }
            None => out.push_str(l),
        }
        out.push('\n');
    }
    out
}

#[test]
fn encodings_match_llvm_mc() {
    let Some(t) = tools(
        "encodings_match_llvm_mc",
        &["llvm-mc", "ld.lld", "llvm-objcopy", "llvm-objdump"],
    ) else {
        return;
    };
    let dir = scratch("llvm_encoding");
    let (mut statements, mut total_words, mut disassembled) = (0, 0, 0);
    for seed in 1..=4u64 {
        let mut rng = Rng::new(0x5EED_0000 + seed);
        let lines = corpus(&mut rng);
        let src = format!("\t.text\n{}\n", lines.join("\n"));
        let name = format!("corpus{seed}");
        let expected = llvm_text_bytes(&dir, &name, &llvm_source(&lines), 0);
        let (p, _) = strata(&[("corpus.s", &src)], LayoutSpec::Org(0));
        let got = common::section_bytes(&p, ".text");
        let (gw, ew) = (words(&got), words(&expected));

        let src_lines: Vec<&str> = src.lines().collect();
        let line_of = |addr: u32| {
            p.asm_lines
                .iter()
                .find(|a| a.addr == addr)
                .map(|a| src_lines.get(a.line as usize - 1).copied().unwrap_or("?"))
        };
        let mut report = String::new();
        let mut mismatches = 0;
        for (i, (g, e)) in gw.iter().zip(&ew).enumerate() {
            if g != e {
                mismatches += 1;
                if mismatches <= 30 {
                    let addr = (i * 4) as u32;
                    let _ = writeln!(
                        report,
                        "  {addr:#06x}: strata {g:08x} llvm {e:08x}  {:?}",
                        line_of(addr)
                    );
                }
            }
        }
        assert_eq!(
            gw.len(),
            ew.len(),
            "seed {seed}: different code sizes\n{report}"
        );
        assert!(
            mismatches == 0,
            "seed {seed}: {mismatches} mismatching words:\n{report}"
        );

        // The disassembler agrees with llvm-objdump on every corpus word.
        let dump = run(Command::new(&t[3])
            .args(["-d", "--mattr=+zicsr,+zifencei"])
            .arg(dir.join(format!("{name}.elf"))));
        let mut bad = String::new();
        for line in dump.lines() {
            let Some((left, text)) = line.split_once('\t') else {
                continue;
            };
            let Some((addr, raw)) = left.split_once(':') else {
                continue;
            };
            let (Ok(addr), Ok(word)) = (
                u32::from_str_radix(addr.trim(), 16),
                u32::from_str_radix(raw.trim(), 16),
            ) else {
                continue;
            };
            let insn = strata_isa::decode(word);
            let ours = normalize_objdump(&strata_isa::disassemble(word, addr));
            if !objdump_text_matches(&ours, &normalize_objdump(text), &insn) {
                let _ = writeln!(bad, "  {word:08x}: strata {ours:?} llvm {text:?}");
            }
            disassembled += 1;
        }
        assert!(bad.is_empty(), "seed {seed}: disassembly differs:\n{bad}");
        statements += lines.iter().filter(|l| !l.ends_with(':')).count();
        total_words += ew.len();
    }
    eprintln!("encoding corpus: {statements} statements, {total_words} instruction words, {disassembled} disassembled");
}

/// Lines both assemblers must reject.
const INVALID: &[&str] = &[
    "addi a0, a0, 2048",
    "addi a0, a0, -2049",
    "andi a0, a0, 0x1000",
    "slli a0, a0, 32",
    "srai a0, a0, -1",
    "lui a0, 0x100000",
    "lui a0, -1",
    "auipc a0, 0x100000",
    "beq a0, a1, 4096",
    "beq a0, a1, -4098",
    "bne a0, a1, 3",
    "jal 1048576",
    "jal x0, -1048578",
    "j 1",
    "csrrwi a0, mstatus, 32",
    "csrrs a0, 4096, a1",
    "csrrs a0, -1, a1",
    "csrw nosuchcsr, a0",
    "lw a0, 2048(a1)",
    "sw a0, -2049(a1)",
    "add a0, a1",
    "add a0, a1, a2, a3",
    "sub a0, a1, 5",
    "lui a0, a0",
    "beq a0, a1, a2",
    "csrr a0, a1",
    "addi a0, a1, a2",
    "addi x32, x0, 1",
    "li a0, 0x100000000",
    "li a0, -0x80000001",
    "fence rw, x",
    "fence wr, r",
    "fence rr, w",
    "fence.i a0",
    "ecall a0",
    "mret 1",
    "foo a0, a1",
    "jalr a0, a1, 2048",
    "lui a0, %lo(5)",
    "addi a0, a0, %hi(5)",
    "lw a0, 16",
    "call 0x100",
    "tail 5",
    "jump 8, t0",
    "sw a0, 16, t0",
    "lb a0, 16",
    "rdcycle",
    "rdtime a0, a1",
    "csrr a0",
    "unimp a0",
    "nop a0",
    "mv a0",
    "not a0, 5",
    "beqz a0",
    "slli a0, a0",
    "sw a0, 4(a1), a2",
];

#[test]
fn invalid_operands_rejected_like_llvm_mc() {
    let Some(t) = tools("invalid_operands_rejected_like_llvm_mc", &["llvm-mc"]) else {
        return;
    };
    let dir = scratch("llvm_invalid");
    let src: String = INVALID.iter().map(|l| format!("{l}\n")).collect();
    let path = dir.join("invalid.s");
    std::fs::write(&path, &src).unwrap();
    let out = Command::new(&t[0])
        .args(MC_ARGS)
        .arg("-filetype=obj")
        .arg(&path)
        .arg("-o")
        .arg(dir.join("x.o"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let prefix = format!("{}:", path.display());
    let llvm_lines: BTreeSet<u32> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix(&prefix))
        .filter(|l| l.contains(": error:"))
        .filter_map(|l| l.split(':').next()?.parse().ok())
        .collect();

    let opts = strata_asm::Options {
        layout: LayoutSpec::Org(0),
        ..Default::default()
    };
    let res = strata_asm::assemble_sources(&[("invalid.s", &src)], &opts);
    let strata_lines: BTreeSet<u32> = res
        .diags
        .list
        .iter()
        .filter_map(|d| d.loc)
        .map(|l| l.line)
        .collect();

    let all: BTreeSet<u32> = (1..=INVALID.len() as u32).collect();
    let mut report = String::new();
    for line in &all {
        let (s, l) = (strata_lines.contains(line), llvm_lines.contains(line));
        if !s || !l {
            let _ = writeln!(
                report,
                "  line {line} {:?}: strata error={s} llvm error={l}",
                INVALID[*line as usize - 1]
            );
        }
    }
    eprintln!("invalid corpus: {} lines", INVALID.len());
    assert!(
        report.is_empty(),
        "every line must be rejected by both assemblers:\n{report}"
    );
    assert!(res.program.is_none());
}
