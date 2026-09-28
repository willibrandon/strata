//! Unit tests of `--gc-sections`: reachability and removal from all outputs.

mod common;

use common::unit::layout;
use strata_asm::link::Program;
use strata_asm::output::{flat_binary, listing, map, write_elf};
use strata_asm::sdb::DebugInfo;
use strata_asm::{assemble_sources, LayoutSpec, Options, Outcome};

const LAYOUT: &str = "
memory M origin=0x1000 length=64K
entry _start
output .text   in M { .text.start .text .text.* }
output .rodata in M { .rodata .rodata.* }
output .data   in M { .data .data.* }
output .bss    in M nobits { .bss .bss.* COMMON }
";

fn run(sources: &[(&str, &str)], layout_text: &str, gc: bool, keep: &[&str]) -> Outcome {
    let opts = Options {
        layout: LayoutSpec::Parsed(layout(layout_text)),
        gc_sections: gc,
        keep_symbols: keep.iter().map(|s| s.to_string()).collect(),
        ..Options::default()
    };
    assemble_sources(sources, &opts)
}

fn gc(sources: &[(&str, &str)], keep: &[&str]) -> Program {
    gc_with(sources, LAYOUT, keep)
}

fn gc_with(sources: &[(&str, &str)], layout_text: &str, keep: &[&str]) -> Program {
    let out = run(sources, layout_text, true, keep);
    match out.program {
        Some(p) => p,
        None => panic!("{}", out.diags.render(&out.files, "strata-as")),
    }
}

/// Names of the removed input sections, as `file(section)`.
fn removed(p: &Program) -> Vec<String> {
    p.gc_removed
        .iter()
        .map(|r| format!("{}({})", r.path, r.name))
        .collect()
}

/// Names of the placed input sections.
fn kept(p: &Program) -> Vec<String> {
    p.sections
        .iter()
        .flat_map(|s| {
            s.inputs
                .iter()
                .filter(|i| i.size > 0)
                .map(|i| i.name.clone())
        })
        .collect()
}

fn has_symbol(p: &Program, name: &str) -> bool {
    p.symbols.iter().any(|s| s.name == name)
}

#[test]
fn chains_are_followed_and_unreferenced_sections_removed() {
    let src = "
        .section .text.start, \"ax\"
        .globl _start
        _start: call a
        .section .text.a, \"ax\"
        a: call b
           ret
        .section .text.b, \"ax\"
        b: j c
        .section .text.c, \"ax\"
        c: ret
        .section .text.unused, \"ax\"
        unused: call a
            ret
    ";
    let p = gc(&[("t.s", src)], &[]);
    assert_eq!(
        kept(&p),
        vec![".text.start", ".text.a", ".text.b", ".text.c"]
    );
    assert_eq!(removed(&p), vec!["t.s(.text.unused)"]);
    assert!(!has_symbol(&p, "unused"));
    assert!(has_symbol(&p, "c"));
    // Without --gc-sections nothing is removed.
    let all = run(&[("t.s", src)], LAYOUT, false, &[]).program.unwrap();
    assert!(all.gc_removed.is_empty());
    assert_eq!(kept(&all).len(), 5);
}

#[test]
fn cycles() {
    let src = "
        .section .text.start, \"ax\"
        .globl _start
        _start: call live1
        .section .text.live1, \"ax\"
        live1: call live2
        .section .text.live2, \"ax\"
        live2: call live1
        .section .text.dead1, \"ax\"
        dead1: call dead2
        .section .text.dead2, \"ax\"
        dead2: call dead1
        .section .text.self, \"ax\"
        self: j self
    ";
    let p = gc(&[("t.s", src)], &[]);
    assert_eq!(kept(&p), vec![".text.start", ".text.live1", ".text.live2"]);
    assert_eq!(
        removed(&p),
        vec!["t.s(.text.dead1)", "t.s(.text.dead2)", "t.s(.text.self)"]
    );
}

#[test]
fn entry_section_is_the_root() {
    // The entry symbol is in a section listed last; nothing else refers to it.
    let src = "
        .section .text.first, \"ax\"
        first: nop
        .section .text.go, \"ax\"
        .globl go
        go: call helper
        .section .text.helper, \"ax\"
        helper: ret
    ";
    let l = LAYOUT.replace("entry _start", "entry go");
    let p = gc_with(&[("t.s", src)], &l, &[]);
    assert_eq!(kept(&p), vec![".text.go", ".text.helper"]);
    assert_eq!(p.entry, 0x1000);
    // --org mode: `_start` is the entry.
    let opts = Options {
        layout: LayoutSpec::Org(0x100),
        gc_sections: true,
        ..Options::default()
    };
    let out = assemble_sources(
        &[(
            "t.s",
            ".section .text.x\nnop\n.section .text.y\n.globl _start\n_start: ret",
        )],
        &opts,
    );
    let p = out.program.unwrap();
    assert_eq!(kept(&p), vec![".text.y"]);
    assert_eq!(p.entry, 0x100);
}

#[test]
fn no_roots_is_an_error() {
    let opts = Options {
        layout: LayoutSpec::Org(0),
        gc_sections: true,
        ..Options::default()
    };
    let out = assemble_sources(&[("t.s", "nop")], &opts);
    let d = out.diags.render(&out.files, "strata-as");
    assert!(out.program.is_none());
    assert!(d.contains("--gc-sections found no roots"), "{d}");
}

#[test]
fn keep_patterns_are_roots_and_place_in_order() {
    let src = "
        .section .text.body, \"ax\"
        .globl _start
        _start: ret
        .section .text.header, \"ax\"
        header: .word magic_table
        .section .rodata.magic, \"a\"
        magic_table: .word 0x5354524A
        .section .rodata.unused, \"a\"
        .word 1
    ";
    let l = LAYOUT.replace(
        "{ .text.start .text .text.* }",
        "{ KEEP(.text.header) .text .text.* }",
    );
    let p = gc_with(&[("t.s", src)], &l, &[]);
    // The header is kept although nothing refers to it, placed first, and
    // what it refers to is kept too.
    assert_eq!(
        kept(&p),
        vec![".text.header", ".text.body", ".rodata.magic"]
    );
    assert_eq!(p.sections[0].inputs[0].name, ".text.header");
    assert_eq!(removed(&p), vec!["t.s(.rodata.unused)"]);
    // KEEP(COMMON) keeps every common symbol.
    let l2 = LAYOUT.replace("COMMON }", "KEEP(COMMON) }");
    let p = gc_with(
        &[("t.s", ".globl _start\n_start: ret\n.comm lonely, 8, 4")],
        &l2,
        &[],
    );
    assert!(has_symbol(&p, "lonely"));
}

#[test]
fn keep_symbol_roots() {
    let src = "
        .globl _start
        _start: ret
        .section .text.api, \"ax\"
        .globl api_entry
        api_entry: call helper
        .section .text.helper, \"ax\"
        helper: ret
        .section .text.local, \"ax\"
        local_fn: ret
        .section .text.other, \"ax\"
        other: ret
    ";
    let p = gc(&[("t.s", src)], &["api_entry", "local_fn"]);
    assert_eq!(
        kept(&p),
        vec![".text", ".text.api", ".text.helper", ".text.local"]
    );
    assert_eq!(removed(&p), vec!["t.s(.text.other)"]);
    let out = run(&[("t.s", src)], LAYOUT, true, &["no_such_symbol"]);
    let d = out.diags.render(&out.files, "strata-as");
    assert!(
        d.contains("--keep-symbol: symbol 'no_such_symbol' is not defined"),
        "{d}"
    );
}

#[test]
fn references_of_every_kind_make_sections_live() {
    let src = "
        .section .text.start, \"ax\"
        .globl _start
        _start:
            la a0, by_pcrel                   # %pcrel_hi/%pcrel_lo
        1:  auipc a1, %pcrel_hi(by_explicit_pcrel)
            addi a1, a1, %pcrel_lo(1b)
            lui a2, %hi(by_hi_lo)
            lw a2, %lo(by_hi_lo)(a2)
            lw a3, by_load                    # load pseudo
            sw a3, by_store, t0               # store pseudo
            beqz a0, by_branch
            la a4, table
            tail by_tail
        .data
        table: .word by_data_word, end_label - start_label, alias
        .section .text.unused, \"ax\"
            call only_from_unused
    ";
    let defs = "
        .section .rodata.a, \"a\"
        by_pcrel: .byte 1
        .section .rodata.b, \"a\"
        by_explicit_pcrel: .byte 2
        .section .rodata.c, \"a\"
        by_hi_lo: .word 3
        .section .data.d
        by_load: .word 4
        .section .data.e
        by_store: .word 5
        .section .text.f, \"ax\"
        by_branch: ret
        .section .text.g, \"ax\"
        by_tail: ret
        .section .rodata.h, \"a\"
        by_data_word: .word 6
        .section .rodata.i, \"a\"
        start_label: .word 7
        .section .rodata.j, \"a\"
        end_label: .word 8
        .section .rodata.k, \"a\"
        alias_target: .word 9, 10
        .section .text.l, \"ax\"
        only_from_unused: ret
    ";
    let src = format!("{src}\n{defs}\n.data\n.globl alias\nalias = alias_target + 4\n");
    let p = gc(&[("t.s", &src)], &[]);
    let removed = removed(&p);
    assert_eq!(removed, vec!["t.s(.text.unused)", "t.s(.text.l)"]);
    for name in [
        "by_pcrel",
        "by_explicit_pcrel",
        "by_hi_lo",
        "by_load",
        "by_store",
        "by_branch",
        "by_tail",
        "by_data_word",
        "start_label",
        "end_label",
        "alias_target",
    ] {
        assert!(has_symbol(&p, name), "{name} was removed");
    }
}

#[test]
fn references_across_files_commons_and_weak_symbols() {
    let a = "
        .globl _start
        _start: call ext_fn
            la a0, shared_common
            call soft
        .comm unused_common, 16, 4
    ";
    let b = "
        .section .text.ext, \"ax\"
        .globl ext_fn
        ext_fn: ret
        .section .text.unused_ext, \"ax\"
        .globl unused_ext
        unused_ext: ret
        .comm shared_common, 8, 4
        .section .text.weak_soft, \"ax\"
        .weak soft
        soft: ret
    ";
    let c = "
        .section .text.strong_soft, \"ax\"
        .globl soft
        soft: ret
    ";
    let p = gc(&[("a.s", a), ("b.s", b), ("c.s", c)], &[]);
    assert!(has_symbol(&p, "ext_fn") && has_symbol(&p, "shared_common"));
    // The strong definition is the one referenced; the weak one's section goes.
    assert_eq!(p.symbol("soft").unwrap().file, Some(2));
    let removed = removed(&p);
    assert!(removed.contains(&"b.s(.text.unused_ext)".to_string()));
    assert!(removed.contains(&"b.s(.text.weak_soft)".to_string()));
    assert!(
        removed.contains(&"COMMON(unused_common)".to_string()),
        "{removed:?}"
    );
    assert!(!has_symbol(&p, "unused_common"));
}

#[test]
fn nobits_sections_participate() {
    let src = "
        .globl _start
        _start: la a0, buf
        .section .bss.buf, \"aw\", @nobits
        buf: .zero 64
        .section .bss.unused, \"aw\", @nobits
        unused_buf: .zero 4096
        .bss
        .lcomm local_unused, 32, 4
    ";
    let p = gc(&[("t.s", src)], &[]);
    let bss = p.sections.iter().find(|s| s.name == ".bss").unwrap();
    assert_eq!(bss.size, 64);
    // `.bss` holds only the unreferenced local common.
    assert_eq!(removed(&p), vec!["t.s(.bss.unused)", "t.s(.bss)"]);
}

#[test]
fn layout_defines_and_section_symbols() {
    let src = "
        .globl _start
        _start: call entry_alias
            la a0, .rodata.named
        .section .text.target, \"ax\"
        .globl target
        target: ret
        .section .rodata.named, \"a\"
        .word 1
    ";
    let l = format!("{LAYOUT}\ndefine entry_alias = target\n");
    let p = gc_with(&[("t.s", src)], &l, &[]);
    assert!(has_symbol(&p, "target"));
    assert!(kept(&p).contains(&".rodata.named".to_string()));
}

#[test]
fn undefined_references_in_removed_sections_are_ignored() {
    let src = "
        .globl _start
        _start: ret
        .section .text.dead, \"ax\"
        dead: call nowhere_defined
        .set dead_alias, also_missing + 4
    ";
    let p = gc(&[("t.s", src)], &[]);
    assert_eq!(removed(&p), vec!["t.s(.text.dead)"]);
    // The same reference from live code is an error.
    let out = run(
        &[("t.s", ".globl _start\n_start: call nowhere_defined")],
        LAYOUT,
        true,
        &[],
    );
    let d = out.diags.render(&out.files, "strata-as");
    assert!(d.contains("undefined symbol 'nowhere_defined'"), "{d}");
}

#[test]
fn removed_sections_are_absent_from_every_output() {
    let src = "
        .file 0 \"/w\" \"m.c\"
        .section .text.start, \"ax\"
        .globl _start
        _start:
        .loc 0 1 1
            call used
        .section .text.used, \"ax\"
        used:
        .loc 0 2 1
            ret
        .section .text.dropped, \"ax\"
        .globl dropped_fn
        dropped_fn:
        .loc 0 3 1
            li a0, 0x123
            ret
        .section .rodata.dropped, \"a\"
        dropped_data: .asciz \"DROPPED\"
    ";
    let out = run(&[("m.s", src)], LAYOUT, true, &[]);
    let p = out.program.unwrap();
    // Bytes: only _start (auipc+jalr) and used (ret).
    let (_, bin) = flat_binary(&p).unwrap();
    assert_eq!(bin.len(), 12);
    assert!(!bin.windows(7).any(|w| w == b"DROPPED"));
    let elf = write_elf(&p);
    assert!(!elf.windows(10).any(|w| w == b"dropped_fn"));
    assert!(!elf.windows(12).any(|w| w == b"dropped_data"));
    // Debug info: no symbols, line rows or instruction rows of removed code.
    let info = DebugInfo::from_program(&p, &out.files);
    assert!(info.symbols.iter().all(|s| !s.name.starts_with("dropped")));
    assert!(info.lines.iter().all(|l| l.2 != 3), "{:?}", info.lines);
    assert_eq!(info.asm_lines.len(), 3);
    // Listing: statements of removed sections are omitted.
    let lst = listing(&p, &out.files);
    assert!(
        !lst.contains("li a0, 0x123") && !lst.contains("DROPPED"),
        "{lst}"
    );
    assert!(lst.contains("call used"));
    // Map: removed sections are listed with their sizes.
    let m = map(&p);
    let tail = m
        .split("Discarded by --gc-sections:")
        .nth(1)
        .expect("heading present");
    assert!(tail.contains("0x00000008  m.s(.text.dropped)"), "{m}");
    assert!(tail.contains("0x00000008  m.s(.rodata.dropped)"), "{m}");
    let before = m.split("Discarded by --gc-sections:").next().unwrap();
    assert!(!before.contains("dropped"), "{m}");
}
