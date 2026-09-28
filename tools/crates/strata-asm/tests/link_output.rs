//! Unit tests of the layout-file linker and the output formats.

mod common;

use common::unit::*;
use strata_asm::layout::parse_layout;
use strata_asm::output::{flat_binary, hex_words, listing, map, write_elf};
use strata_asm::sdb::DebugInfo;
use strata_asm::{assemble_sources, LayoutSpec, Options};

const KERNEL_LD: &str = "# comments start with '#'
memory RAM  origin=0x10000000 length=16K
entry _start

output .text   in RAM { .text.start .text .text.* }
output .rodata in RAM align=4 { .rodata .rodata.* .srodata .srodata.* }
output .data   in RAM align=4 { .data .data.* .sdata .sdata.* }
output .bss    in RAM align=4 nobits { .bss .bss.* .sbss .sbss.* COMMON }

define __bss_start = START(.bss)
define __bss_end   = END(.bss)
define __stack_top = ORIGIN(RAM) + LENGTH(RAM)
define __text_size = SIZE(.text)
define __aligned   = ALIGN(__bss_end + 1, 256) | 1
";

const START_S: &str = "
    .section .text.start, \"ax\", @progbits
    .globl _start
_start:
    la sp, __stack_top
    la a0, __bss_start
    la a1, __bss_end
1:  bgeu a0, a1, 2f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 1b
2:  call main
    j .
    .data
    .word __text_size, __aligned
";

const MAIN_S: &str = "
    .text
    .globl main
    .type main, @function
main:
    lui a0, %hi(msg)
    addi a0, a0, %lo(msg)
    lw a1, counter
    ret
    .size main, . - main
    .section .rodata.str1.1, \"aMS\", @progbits, 1
msg: .asciz \"hi\"
    .data
    .p2align 2
counter: .word 7
    .bss
buf: .zero 10
    .comm pool, 64, 16
";

#[test]
fn spec_example_layout() {
    let p = progs(
        &[("start.s", START_S), ("main.s", MAIN_S)],
        LayoutSpec::Parsed(layout(KERNEL_LD)),
    );
    let names: Vec<(&str, u32, u32, bool)> = p
        .sections
        .iter()
        .map(|s| (s.name.as_str(), s.addr, s.size, s.nobits))
        .collect();
    // .text.start comes first even though main.s's .text is listed later on the
    // pattern list's second entry; .rodata/.data/.bss follow in order.
    assert_eq!(names[0].0, ".text");
    assert_eq!(sym(&p, "_start"), 0x1000_0000);
    assert!(sym(&p, "main") > sym(&p, "_start"));
    let text = &p.sections[0];
    assert_eq!(text.inputs[0].name, ".text.start");
    assert_eq!(p.entry, 0x1000_0000);
    let (rodata, data, bss) = (&p.sections[1], &p.sections[2], &p.sections[3]);
    assert_eq!(rodata.addr, text.addr + text.size);
    assert_eq!(rodata.addr % 4, 0);
    assert!(data.addr >= rodata.addr + rodata.size && data.addr % 4 == 0);
    assert!(
        bss.nobits && bss.addr % 16 == 0,
        "COMMON alignment raises the section alignment"
    );
    assert_eq!(sym(&p, "__bss_start"), bss.addr);
    assert_eq!(sym(&p, "__bss_end"), bss.addr + bss.size);
    assert_eq!(sym(&p, "__stack_top"), 0x1000_4000);
    let pool = p.symbol("pool").unwrap();
    assert_eq!((pool.size, pool.addr % 16), (64, 0));
    assert!(pool.addr >= sym(&p, "buf") + 10);
    // Layout symbols are absolute and global.
    let top = p.symbol("__stack_top").unwrap();
    assert_eq!(
        (top.section, top.bind),
        (None, strata_asm::object::Bind::Global)
    );
    let d = section_words(&p, ".data");
    assert_eq!(d[0], text.size);
    assert_eq!(d[1], ((sym(&p, "__bss_end") + 1 + 255) & !255) | 1);
}

#[test]
fn at_align_and_regions() {
    let l = layout(
        "memory ROM origin=0 length=4K
         memory RAM origin=0x2000 length=0x100
         output .text in ROM { .text }
         output .vectors in ROM at=0x800 align=64 { .vectors }
         output .rodata in ROM { .rodata }
         output .data in RAM { .data }
         entry start",
    );
    let p = progs(
        &[("a.s", ".globl start\nstart: nop\n.section .vectors, \"ax\"\nj start\n.section .rodata, \"a\"\n.byte 1\n.data\n.word 2")],
        LayoutSpec::Parsed(l),
    );
    let get = |n: &str| p.sections.iter().find(|s| s.name == n).unwrap().clone();
    assert_eq!(get(".text").addr, 0);
    assert_eq!((get(".vectors").addr, get(".vectors").align), (0x800, 64));
    assert_eq!(
        get(".rodata").addr,
        0x804,
        "continues after the at= section"
    );
    assert_eq!(get(".data").addr, 0x2000);
    let (base, bin) = flat_binary(&p).unwrap();
    assert_eq!(base, 0);
    assert_eq!(bin.len(), 0x2004, "gaps are zero-filled");
    assert_eq!(&bin[0x2000..], &[2, 0, 0, 0]);
    assert_eq!(&bin[0x804..0x805], &[1]);
    assert!(bin[0x808..0x1000].iter().all(|&b| b == 0));
}

#[test]
fn layout_errors() {
    let small = "memory RAM origin=0x100 length=8\noutput .text in RAM { .text }\n";
    let d = diags_with(
        &[("t.s", "nop\nnop\nnop")],
        LayoutSpec::Parsed(layout(small)),
    );
    assert!(d.contains("overflows region 'RAM' by 4 bytes"), "{d}");
    let d = diags_with(
        &[("t.s", ".data\n.word 1")],
        LayoutSpec::Parsed(layout(small)),
    );
    assert!(
        d.contains("section '.data' (4 bytes) matches no output section"),
        "{d}"
    );
    // Empty unmatched sections are fine.
    let _ = progs(
        &[("t.s", "nop\n.data\n")],
        LayoutSpec::Parsed(layout(small)),
    );
    let d = diags_with(&[("t.s", ".comm c, 4")], LayoutSpec::Parsed(layout(small)));
    assert!(d.contains("no output section lists COMMON"), "{d}");
    let two = "memory A origin=0 length=64\noutput .text in A { .text }\noutput .data in A at=0 { .data }\n";
    let d = diags_with(
        &[("t.s", "nop\n.data\n.word 1")],
        LayoutSpec::Parsed(layout(two)),
    );
    assert!(d.contains("overlap"), "{d}");
    let nb = "memory A origin=0 length=64\noutput .data in A nobits { .data }\n";
    let d = diags_with(&[("t.s", ".data\n.word 1")], LayoutSpec::Parsed(layout(nb)));
    assert!(
        d.contains("has contents but output section '.data' is nobits"),
        "{d}"
    );
    let ent = "memory A origin=0 length=64\noutput .text in A { .text }\nentry nowhere\n";
    let d = diags_with(&[("t.s", "nop")], LayoutSpec::Parsed(layout(ent)));
    assert!(d.contains("entry symbol 'nowhere' is not defined"), "{d}");
    let def = "memory A origin=0 length=64\noutput .text in A { .text }\ndefine x = START(.nope)\ndefine main = 4\n";
    let d = diags_with(
        &[("t.s", ".globl main\nmain: nop")],
        LayoutSpec::Parsed(layout(def)),
    );
    assert!(
        d.contains("unknown output section '.nope'")
            && d.contains("defined by the layout and by an input file"),
        "{d}"
    );
    // Syntax errors in the layout file.
    let mut dg = strata_asm::diag::Diags::new();
    assert!(parse_layout("memory R origin=1\n", strata_asm::diag::FileId(0), &mut dg).is_none());
    assert!(parse_layout(
        "output .t in R { .t }\n",
        strata_asm::diag::FileId(0),
        &mut dg
    )
    .is_none());
    assert!(parse_layout(
        "memory R origin=0 length=4\noutput .t in R { .t\n",
        strata_asm::diag::FileId(0),
        &mut dg
    )
    .is_none());
}

#[test]
fn symbol_resolution_across_files() {
    let p = progs(
        &[
            ("a.s", ".globl shared\n.weak soft\nsoft: .word 1\nhelper: .word 2\n.data\nshared: .word soft, helper, other"),
            ("b.s", ".globl soft, other\nsoft: .word 3\nhelper: .word 4\nother: .word shared"),
        ],
        LayoutSpec::Org(0),
    );
    // The strong definition in b.s wins over the weak one in a.s.
    let soft = p.symbol("soft").unwrap();
    assert_eq!(soft.addr, 8);
    assert_eq!(soft.file, Some(1));
    // Each file has its own local `helper`.
    assert_eq!(p.symbols.iter().filter(|s| s.name == "helper").count(), 2);
    let data = section_words(&p, ".data");
    assert_eq!(data, vec![8, 4, sym(&p, "other")]);
    let d = diags_with(
        &[("a.s", ".globl x\nx: nop"), ("b.s", ".globl x\nx: nop")],
        LayoutSpec::Org(0),
    );
    assert!(
        d.contains("b.s:2:1: error: multiple definition of 'x'")
            && d.contains("a.s:2:1: note: previous definition"),
        "{d}"
    );
    let d = diags_with(
        &[("a.s", "call from_b"), ("b.s", "from_b: ret")],
        LayoutSpec::Org(0),
    );
    assert!(
        d.contains("undefined symbol 'from_b'"),
        "a local symbol is not visible to other files:\n{d}"
    );
}

#[test]
fn org_mode_entry_and_order() {
    let p = progs(
        &[("a.s", ".section .text.b\nnop\n.text\nnop\n.globl _start\n_start: nop\n.bss\n.zero 4\n.data\n.byte 1\n.section .srodata\n.byte 2\n.rodata\n.byte 3")],
        LayoutSpec::Org(0x8000),
    );
    let order: Vec<&str> = p.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(order, vec![".text", ".rodata", ".data", ".bss"]);
    assert_eq!(p.entry, 0x8004);
    assert_eq!(
        p.sections[0]
            .inputs
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        vec![".text", ".text.b"]
    );
    assert_eq!(section_bytes(&p, ".rodata"), vec![3, 2]);
    let p = prog_at("nop", 0x8000);
    assert_eq!(
        p.entry, 0x8000,
        "without _start the entry is the start of the first section"
    );
}

// ----- outputs -------------------------------------------------------------------------------------

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn cstr(b: &[u8], o: usize) -> String {
    let end = b[o..]
        .iter()
        .position(|&c| c == 0)
        .map_or(b.len(), |e| o + e);
    String::from_utf8_lossy(&b[o..end]).into_owned()
}

#[test]
fn elf_structure() {
    let p = progs(
        &[("start.s", START_S), ("main.s", MAIN_S)],
        LayoutSpec::Parsed(layout(KERNEL_LD)),
    );
    let elf = write_elf(&p);
    assert_eq!(&elf[..6], &[0x7F, b'E', b'L', b'F', 1, 1]);
    assert_eq!(u16_at(&elf, 16), 2, "ET_EXEC");
    assert_eq!(u16_at(&elf, 18), 243, "EM_RISCV");
    assert_eq!(u32_at(&elf, 24), p.entry);
    let (phoff, shoff) = (u32_at(&elf, 28) as usize, u32_at(&elf, 32) as usize);
    let (phnum, shnum, shstrndx) = (
        u16_at(&elf, 44) as usize,
        u16_at(&elf, 48) as usize,
        u16_at(&elf, 50) as usize,
    );
    let loaded: Vec<_> = p.sections.iter().filter(|s| s.size > 0).collect();
    assert_eq!(phnum, loaded.len());
    assert_eq!(shnum, loaded.len() + 4);
    let sh = |i: usize, f: usize| u32_at(&elf, shoff + 40 * i + 4 * f);
    let strtab_off = sh(shstrndx, 4) as usize;
    let mut names = Vec::new();
    for i in 1..shnum {
        names.push(cstr(&elf, strtab_off + sh(i, 0) as usize));
    }
    assert_eq!(
        names,
        vec![
            ".text",
            ".rodata",
            ".data",
            ".bss",
            ".symtab",
            ".strtab",
            ".shstrtab"
        ]
    );
    for (i, s) in loaded.iter().enumerate() {
        assert_eq!((sh(i + 1, 3), sh(i + 1, 5)), (s.addr, s.size));
        let off = sh(i + 1, 4) as usize;
        if !s.nobits {
            assert_eq!(
                &elf[off..off + s.data.len()],
                &s.data[..],
                "{} contents",
                s.name
            );
        }
        let ph = |f: usize| u32_at(&elf, phoff + 32 * i + 4 * f);
        assert_eq!(
            (ph(0), ph(1), ph(2), ph(5)),
            (1, off as u32, s.addr, s.size)
        );
    }
    // Symbol table: locals before globals, names resolvable.
    let symtab = 1 + loaded.len();
    let (so, ssz, first_global) = (
        sh(symtab, 4) as usize,
        sh(symtab, 5) as usize,
        sh(symtab, 7) as usize,
    );
    let str_off = sh(symtab + 1, 4) as usize;
    let mut seen_global = false;
    let mut found_main = false;
    for k in 1..ssz / 16 {
        let e = so + 16 * k;
        let bind = elf[e + 12] >> 4;
        if k >= first_global {
            seen_global = true;
            assert_ne!(bind, 0);
        } else {
            assert_eq!(bind, 0);
        }
        if cstr(&elf, str_off + u32_at(&elf, e) as usize) == "main" {
            found_main = true;
            assert_eq!(
                (u32_at(&elf, e + 4), elf[e + 12] & 15),
                (sym(&p, "main"), 2)
            );
        }
    }
    assert!(seen_global && found_main);
}

#[test]
fn bin_and_hex() {
    let p = prog_at("nop\n.data\n.byte 0xAA\n.bss\n.zero 100", 0x100);
    let (base, bin) = flat_binary(&p).unwrap();
    assert_eq!(base, 0x100);
    assert_eq!(
        bin,
        vec![0x13, 0, 0, 0, 0xAA],
        ".bss is not part of the image"
    );
    assert_eq!(hex_words(&bin), "00000013\n000000aa\n");
}

#[test]
fn debug_info_json() {
    let out = assemble_sources(
        &[("start.s", START_S), ("main.s", MAIN_S)],
        &Options {
            layout: LayoutSpec::Parsed(layout(KERNEL_LD)),
            ..Options::default()
        },
    );
    let p = out.program.unwrap();
    let json = DebugInfo::from_program(&p, &out.files).to_json();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    for k in [
        "format",
        "version",
        "entry",
        "sections",
        "symbols",
        "files",
        "lines",
        "asm_lines",
    ] {
        assert!(keys.contains(&k), "missing key {k}");
    }
    assert_eq!(v["format"], "strata-debug");
    assert_eq!(v["version"], 1);
    assert_eq!(v["entry"], 0x1000_0000);
    assert_eq!(v["files"][0], "start.s");
    assert_eq!(v["files"][1], "main.s");
    let text = &v["sections"][0];
    assert_eq!(
        (
            text["name"].as_str(),
            text["flags"].as_str(),
            text["nobits"].as_bool()
        ),
        (Some(".text"), Some("ax"), Some(false))
    );
    let main = v["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "main")
        .unwrap();
    assert_eq!(main["kind"], "func");
    assert_eq!(main["bind"], "global");
    assert_eq!(main["section"], ".text");
    assert_eq!(main["file"], 1);
    assert_eq!(main["size"], 20);
    let layout_sym = v["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "__stack_top")
        .unwrap();
    assert!(layout_sym["section"].is_null() && layout_sym["file"].is_null());
    // Local labels (.L*, numeric) are omitted; other locals are kept.
    let names: Vec<&str> = v["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert!(names.contains(&"msg") && names.contains(&"buf"));
    assert!(!names
        .iter()
        .any(|n| n.starts_with(".L") || n.contains('\u{2}')));
    for row in v["asm_lines"].as_array().unwrap() {
        assert_eq!(row.as_array().unwrap().len(), 3);
    }
    let first = &v["asm_lines"][0];
    assert_eq!(
        (first[0].as_u64(), first[1].as_u64(), first[2].as_u64()),
        (Some(0x1000_0000), Some(0), Some(5))
    );
    // Round trip through the reader.
    let back = DebugInfo::from_json(&json).unwrap();
    assert_eq!(back, DebugInfo::from_program(&p, &out.files));
    assert_eq!(back.symbol_addr("main").unwrap(), sym(&p, "main"));
    assert!(DebugInfo::from_json("{\"format\":\"x\"}").is_err());
}

#[test]
fn listing_and_map() {
    let out = assemble_sources(
        &[("start.s", START_S), ("main.s", MAIN_S)],
        &Options {
            layout: LayoutSpec::Parsed(layout(KERNEL_LD)),
            ..Options::default()
        },
    );
    let p = out.program.unwrap();
    let lst = listing(&p, &out.files);
    assert!(lst.contains("# start.s") && lst.contains("# main.s"));
    // `la sp, __stack_top` expands to two words at consecutive addresses.
    let la = lst
        .lines()
        .position(|l| l.contains("la sp, __stack_top"))
        .unwrap();
    let line: Vec<&str> = lst.lines().nth(la).unwrap().split_whitespace().collect();
    assert_eq!(&line[..2], &["5", "10000000"]);
    let next: Vec<&str> = lst
        .lines()
        .nth(la + 1)
        .unwrap()
        .split_whitespace()
        .collect();
    assert_eq!(next[0], "10000004");
    assert!(lst.contains("68 69 00"), "string bytes are listed:\n{lst}");
    let m = map(&p);
    for needle in [
        "RAM",
        ".text.start",
        "main.s(.rodata.str1.1)",
        "COMMON(COMMON)",
        "__stack_top",
        "pool",
    ] {
        assert!(m.contains(needle), "map lacks {needle}:\n{m}");
    }
}
