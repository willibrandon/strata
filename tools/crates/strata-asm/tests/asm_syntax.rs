//! Unit tests of source syntax, expressions and directives.

mod common;

use common::unit::*;
use strata_asm::sdb::DebugInfo;
use strata_asm::{assemble_sources, LayoutSpec, Options};

const NOP: u32 = 0x0000_0013;

// ----- statements, comments, labels -------------------------------------------------------

#[test]
fn statements_comments_and_labels() {
    let words = text(
        "a: b: nop ; addi a0, a0, 1 # comment ; not a statement
         /* block
            comment ; nop */ nop /* inline */ ; nop
         c:
         .word a, b, c",
    );
    assert_eq!(words, vec![NOP, 0x0015_0513, NOP, NOP, 0, 0, 16]);
}

#[test]
fn character_literals_and_strings() {
    let p = prog(".data\n.byte 'a', '\\n', '\\t', '\\\\', '\\'', '\\0', ';', '#'\n.ascii \"x;#\\\"\\101\\x42\"");
    assert_eq!(section_bytes(&p, ".data"), b"a\n\t\\'\0;#x;#\"AB".to_vec());
}

#[test]
fn identifiers_may_contain_dots_and_dollars() {
    let p = prog(".globl a.b$c\na.b$c: nop\n.L.x$1: .word a.b$c, .L.x$1");
    assert_eq!(sym(&p, "a.b$c"), 0);
    assert_eq!(section_words(&p, ".text"), vec![NOP, 0, 4]);
}

#[test]
fn line_markers_set_file_and_line_for_diagnostics() {
    let d = diags("nop\n# 42 \"foo.S\" 1\nnop\nbogus\n# 7 \"bar.S\"\n\n  addi a0, a0, 5000\n");
    assert!(
        d.contains("foo.S:43:1: error: unknown instruction 'bogus'"),
        "{d}"
    );
    assert!(d.contains("bar.S:8:16: error:"), "{d}");
}

#[test]
fn line_markers_drive_asm_lines() {
    let p = prog("# 10 \"src.S\"\nnop\nnop\n# 1 \"inc.h\"\nnop\n");
    let info = DebugInfo::from_program(&p, &files_for(&p));
    let _ = info;
    let rows: Vec<(u32, u32)> = p.asm_lines.iter().map(|a| (a.addr, a.line)).collect();
    assert_eq!(rows, vec![(0, 10), (4, 11), (8, 1)]);
}

/// FileTable matching `prog` (single source named t.s).
fn files_for(_p: &strata_asm::link::Program) -> strata_asm::diag::FileTable {
    let mut f = strata_asm::diag::FileTable::new();
    f.intern("t.s");
    f
}

#[test]
fn error_locations_have_line_and_column() {
    let d = diags("nop\n  addi a0, a0, 4096\n\tli a0\n");
    assert!(d.contains("t.s:2:16: error: operand must be"), "{d}");
    assert!(d.contains("t.s:3:7: error: too few operands"), "{d}");
}

// ----- expressions ------------------------------------------------------------------------------

#[test]
fn expression_operators_and_precedence() {
    let p = prog(
        ".data
         .word 1 + 2 * 3 << 1, (1 | 2 ^ 3 & 1), -7 / 2, -7 % 2, ~0 & 0xff
         .word 0b1010, 017, 'A' + 1, !0 + !5, 3 > 2, 2 >= 3, 1 == 1, 1 != 1
         .word -1 >> 60, 5 && 0, 5 || 0, (2 <= 2) + (1 < 0)",
    );
    let w = section_words(&p, ".data");
    assert_eq!(w[..5], [14, 3, (-3i32) as u32, (-1i32) as u32, 255]);
    assert_eq!(w[5..13], [10, 15, 66, 1, u32::MAX, 0, u32::MAX, 0]);
    // `>>` is a logical shift of the 64-bit value, as in GNU as and LLVM.
    assert_eq!(w[13..], [15, 0, 1, u32::MAX]);
}

#[test]
fn dot_and_label_differences() {
    let p = prog(
        "start: nop
         here: .word . - start, here - start, end - start
         .word . + 4
         end:",
    );
    assert_eq!(section_words(&p, ".text"), vec![NOP, 4, 4, 20, 20]);
}

#[test]
fn numeric_local_labels() {
    let words = text(
        "1: nop
            j 1b
            j 1f
         1: j 1b
            j 2f
         2: j 2b",
    );
    // j offsets: -4, +8, 0, +4, 0
    let j = |off: i32| {
        strata_isa::encode(&strata_isa::Insn {
            imm: off,
            ..strata_isa::Insn::new(strata_isa::Op::Jal)
        })
        .unwrap()
    };
    assert_eq!(words, vec![NOP, j(-4), j(4), j(0), j(4), j(0)]);
    assert_error("j 3b", "no previous definition of local label '3'");
    assert_error("j 4f\nnop", "undefined local label '4f'");
}

#[test]
fn absolute_expression_errors() {
    assert_error(".data\n.word 1 / 0", "division by zero");
    assert_error(".data\nx: .word x + x", "cannot add two relocatable values");
    assert_error(".data\nx: .word x * 2", "requires absolute operands");
    assert_error(".data\n.byte 256", "does not fit in 1 byte");
    assert_error(".data\n.half -32769", "does not fit in 2 byte");
    assert_error(".data\n.word 0x100000000", "does not fit in 4 byte");
    assert_error(
        ".data\n.word 0x10000000000000000",
        "integer literal too large",
    );
    assert_error(
        ".data\nx: .byte x",
        "relocatable expression not allowed in 1-byte data",
    );
    assert_error(
        "li a0, later\nlater:",
        "must be an absolute value known at this point",
    );
}

// ----- sections ---------------------------------------------------------------------------------

#[test]
fn section_switching_and_stack() {
    let p = prog(
        ".text
         nop
         .data
         .byte 1
         .pushsection .rodata.x, \"a\", @progbits
         .byte 2
         .popsection
         .byte 3
         .section .text.hot, \"ax\", %progbits
         nop
         .previous
         .byte 4
         .section \".rodata.y\",\"a\"
         .byte 5
         .bss
         .zero 8",
    );
    assert_eq!(section_bytes(&p, ".data"), vec![1, 3, 4]);
    assert_eq!(section_bytes(&p, ".rodata"), vec![2, 5]);
    let bss = p.sections.iter().find(|s| s.name == ".bss").unwrap();
    assert!(bss.nobits && bss.size == 8);
    let t = p.sections.iter().find(|s| s.name == ".text").unwrap();
    assert!(t.flags.exec && t.flags.alloc && !t.flags.write);
    assert_error(".popsection", ".popsection without matching .pushsection");
    assert_error(".section .x, \"a\", @bogus", "unknown section type");
    assert_error(".text 1", "subsections are not supported");
}

#[test]
fn nobits_sections() {
    let p = prog(".section .bss.mine, \"aw\", @nobits\n.zero 16\n.section .sbss\n.zero 4\n");
    assert!(p.sections.iter().all(|s| s.data.is_empty()));
    assert_error(".bss\n.byte 1", "non-zero data in nobits section");
    assert_error(".bss\nnop", "instruction in nobits section");
    assert_error(".section .sbss.x\n.word 5", "non-zero data");
}

#[test]
fn discarded_sections_drop_contents_and_references() {
    let p = prog(
        ".text
         nop
         .section .debug_info, \"\", @progbits
         .Linfo: .word .Lundefined_anywhere - .Linfo
         .uleb128 12345
         .byte 1, 2, 3
         bogus_insn a0
         .section .comment
         .asciz \"x\"
         .section .note.GNU-stack, \"\", @progbits
         .section .riscv.attributes
         .text
         nop",
    );
    assert_eq!(section_words(&p, ".text"), vec![NOP, NOP]);
    assert_eq!(p.sections.iter().filter(|s| s.size > 0).count(), 1);
}

// ----- data -------------------------------------------------------------------------------------

#[test]
fn data_directives() {
    let p = prog(
        ".data
         .byte 1, -1, 0xff
         .2byte 0x1234
         .half -2
         .short 3
         .4byte 0x12345678
         .long -1
         .word 7
         .8byte 0x1122334455667788
         .quad -2
         .dword 1
         .ascii \"ab\", \"c\"
         .asciz \"de\"
         .string \"f\"
         .uleb128 624485
         .sleb128 -123456",
    );
    let mut e: Vec<u8> = vec![1, 0xff, 0xff, 0x34, 0x12, 0xfe, 0xff, 3, 0];
    e.extend([0x78, 0x56, 0x34, 0x12, 0xff, 0xff, 0xff, 0xff, 7, 0, 0, 0]);
    e.extend(0x1122334455667788u64.to_le_bytes());
    e.extend((-2i64).to_le_bytes());
    e.extend(1u64.to_le_bytes());
    e.extend(b"abcde\0f\0");
    e.extend([0xe5, 0x8e, 0x26]);
    e.extend([0xc0, 0xbb, 0x78]);
    assert_eq!(section_bytes(&p, ".data"), e);
}

#[test]
fn fill_directives() {
    let p = prog(
        ".data
         .zero 3
         .space 2, 0xaa
         .skip 1, 7
         .fill 2, 2, 0x0102
         .fill 3
         .fill 1, 8, -1",
    );
    let mut e = vec![0, 0, 0, 0xaa, 0xaa, 7, 2, 1, 2, 1, 0, 0, 0];
    e.extend([0xff; 8]);
    assert_eq!(section_bytes(&p, ".data"), e);
    assert_error(".data\n.space -1", "size must not be negative");
    assert_error(".data\n.space later\nlater:", "known at this point");
}

#[test]
fn org_directive() {
    let p = prog(".data\n.byte 1\n.org 4, 0xee\n.byte 2\nx: .org x + 2\n.byte 3");
    assert_eq!(
        section_bytes(&p, ".data"),
        vec![1, 0xee, 0xee, 0xee, 2, 0, 0, 3]
    );
    assert_error(
        ".data\n.byte 1, 2\n.org 1",
        "cannot move the location counter backwards",
    );
}

// ----- alignment ------------------------------------------------------------------------------

#[test]
fn alignment_in_code_uses_nops() {
    let p = prog("nop\n.p2align 4\nnop\n.byte 1\n.align 4\n.balign 4\n");
    let words = section_words(&p, ".text");
    // A nop, 3 nops of padding to 16, a nop, one byte, then 3 zero bytes and
    // 2 nops to reach 32.
    assert_eq!(words, vec![NOP, NOP, NOP, NOP, NOP, 0x0000_0001, NOP, NOP]);
    let t = p.sections.iter().find(|s| s.name == ".text").unwrap();
    assert_eq!(t.align, 16);
}

#[test]
fn alignment_fill_and_max() {
    let p = prog(
        ".data
         .byte 1
         .p2align 2, 0x55
         .byte 2
         .p2align 3, , 2
         .byte 3
         .balign 8, 0x66, 7
         .byte 4
         .align 1",
    );
    assert_eq!(
        section_bytes(&p, ".data"),
        vec![1, 0x55, 0x55, 0x55, 2, 3, 0x66, 0x66, 4, 0]
    );
    // Explicit fill in code is honored.
    assert_eq!(text("nop\n.p2align 3, 0x00\n.word 1"), vec![NOP, 0, 1]);
    assert_error(".balign 3", "power of two");
    assert_error(".p2align 31", "between 0 and 30");
    assert_error(".p2align 2, 300", "fill value must fit in a byte");
}

#[test]
fn alignment_in_data_of_exec_section_is_nops() {
    // A section with the x flag gets nop padding even without instructions.
    let p = prog(".section .text.table, \"ax\", @progbits\n.byte 1\n.p2align 3\n.word 2");
    assert_eq!(section_words(&p, ".text"), vec![0x1, NOP, 2]);
}

// ----- symbols ------------------------------------------------------------------------------------

#[test]
fn equ_set_and_assignment() {
    let p = prog(
        ".equ A, 5
         .set B, A * 2
         C = B + 1
         .data
         .word A, B, C
         .set B, 100
         .word B
         .set x, 1
         .set x, x + 1
         .word x
         .equ fwd_user, later - here
         here: .word fwd_user
         later:
         .equiv D, 9
         .word D",
    );
    assert_eq!(section_words(&p, ".data"), vec![5, 10, 11, 100, 2, 4, 9]);
    assert_error(".equiv E, 1\n.equiv E, 2", "already defined");
    assert_error("lbl: nop\n.set lbl, 3", "already defined");
    assert_error("lbl: nop\nlbl: nop", "symbol 'lbl' is already defined");
}

#[test]
fn equ_to_label_defines_alias_symbol() {
    let p = prog(
        ".data
         .globl alias
         .type base, @object
         base: .word 1, 2
         alias = base + 4
         .size alias, 4",
    );
    let s = p.symbol("alias").unwrap();
    assert_eq!(
        (s.addr, s.size, s.kind),
        (4, 4, strata_asm::object::SymKind::Object)
    );
    assert_eq!(p.sections[s.section.unwrap()].name, ".data");
}

#[test]
fn symbol_binding_type_and_size() {
    let p = prog(
        ".globl f, g
         .weak w
         .type f, @function
         .type g, %function
         .type obj, STT_OBJECT
         f: nop
         nop
         .size f, . - f
         g: ret
         .size g, 4
         w: nop
         local_fn: nop
         .data
         obj: .word 0
         .size obj, 4",
    );
    let get = |n: &str| p.symbol(n).unwrap().clone();
    use strata_asm::object::{Bind, SymKind};
    assert_eq!(
        (get("f").bind, get("f").kind, get("f").size),
        (Bind::Global, SymKind::Func, 8)
    );
    assert_eq!(
        (get("g").bind, get("g").kind, get("g").size),
        (Bind::Global, SymKind::Func, 4)
    );
    assert_eq!(get("w").bind, Bind::Weak);
    assert_eq!(
        (get("local_fn").bind, get("local_fn").kind),
        (Bind::Local, SymKind::NoType)
    );
    assert_eq!((get("obj").kind, get("obj").size), (SymKind::Object, 4));
    assert_error(".type f, @tls_object", "unsupported symbol type");
}

#[test]
fn commons() {
    let p = progs(
        &[
            ("a.s", ".comm shared, 8, 8\n.comm small, 3\n.lcomm priv, 12, 4\n.local lc\n.comm lc, 4, 4\n.data\n.word shared, priv, lc"),
            ("b.s", ".comm shared, 32, 4\n.bss\n.byte 0\n"),
        ],
        LayoutSpec::Org(0x1000),
    );
    let shared = p.symbol("shared").unwrap();
    assert_eq!(
        (shared.size, shared.addr % 8),
        (32, 0),
        "merged common: max size, max align"
    );
    let bss = p.sections.iter().find(|s| s.name == ".bss").unwrap();
    // a.s .bss (priv 12 + lc 4 = 16), b.s .bss (1), COMMON (small 3 aligned 2, shared 32 aligned 8).
    assert!(bss.inputs.len() >= 3);
    assert!(p
        .symbols
        .iter()
        .any(|s| s.name == "priv" && s.bind == strata_asm::object::Bind::Local));
    let data = section_words(&p, ".data");
    assert_eq!(data, vec![shared.addr, sym(&p, "priv"), sym(&p, "lc")]);
    // A definition wins over a common symbol.
    let p = progs(
        &[
            ("a.s", ".comm x, 4"),
            ("b.s", ".data\n.globl x\nx: .word 7"),
        ],
        LayoutSpec::Org(0),
    );
    assert_eq!(
        p.symbol("x")
            .unwrap()
            .section
            .map(|i| p.sections[i].name.clone()),
        Some(".data".into())
    );
}

// ----- macros, repetition, conditionals -----------------------------------------------------------

#[test]
fn macros_with_defaults_keywords_varargs_and_counter() {
    let words = text(
        ".macro ADDN reg, n=1
            addi \\reg, \\reg, \\n
         .endm
         .macro LBL
         lbl\\@: .word lbl\\@
         .endm
         .macro SUM first, rest:vararg
            .word \\first
            .ifnb \\rest
            SUM \\rest
            .endif
         .endm
         .macro NEED x:req
            .word \\x
         .endm
         ADDN a0
         ADDN a1, 5
         ADDN n=7, reg=a2
         ADDN a3 2
         LBL
         LBL
         SUM 1, 2, 3
         NEED 9
         .macro CAT a, b
            .word \\a\\()\\b
         .endm
         CAT 1, 2",
    );
    let addi = |rd: u8, imm: i32| {
        strata_isa::encode(&strata_isa::Insn {
            rd,
            rs1: rd,
            imm,
            ..strata_isa::Insn::new(strata_isa::Op::Addi)
        })
        .unwrap()
    };
    assert_eq!(
        words,
        vec![
            addi(10, 1),
            addi(11, 5),
            addi(12, 7),
            addi(13, 2),
            16,
            20,
            1,
            2,
            3,
            9,
            12
        ]
    );
}

#[test]
fn macro_exitm_nesting_and_errors() {
    let words = text(
        ".macro M n
            .word \\n
            .if \\n > 2
            .exitm
            .endif
            .word 100
         .endm
         M 1
         M 3
         .macro OUTER
            .macro INNER
               .word 7
            .endm
            INNER
         .endm
         OUTER",
    );
    assert_eq!(words, vec![1, 100, 3, 7]);
    assert_error(".macro X\nnop\n", "missing .endm");
    assert_error(".endm", "without matching .macro");
    assert_error(".exitm", "outside of a macro");
    assert_error(".macro R\nR\n.endm\nR", "expanded recursively too deeply");
    assert_error(
        ".macro Q a:req\n.word \\a\n.endm\nQ",
        "missing value for required parameter",
    );
    assert_error(".macro Z a\n.endm\nZ 1, 2", "too many arguments");
    let d = diags(".macro BAD\naddi a0, a0, 9999\n.endm\nnop\nBAD\n");
    assert!(
        d.contains("t.s:5:1: error:") && d.contains("in expansion of macro 'BAD'"),
        "{d}"
    );
    assert!(!diags(".macro P\n.endm\n.purgem P\n.macro P\n.endm").contains("error"));
}

#[test]
fn rept_irp_irpc() {
    let words = text(
        ".rept 3
         .word 1
         .endr
         .irp r, 4, 5, 6
         .word \\r
         .endr
         .irpc c, 78
         .word \\c
         .endr
         .rept 2
         .irp x, 0
         .word \\x + 10
         .endr
         .endr
         .rept 0
         .word 99
         .endr",
    );
    assert_eq!(words, vec![1, 1, 1, 4, 5, 6, 7, 8, 10, 10]);
    assert_error(".rept 2\nnop", ".rept is missing .endr");
    assert_error(".endr", "without matching .rept");
    assert_error(".rept -1\n.endr", "must not be negative");
    assert_error(".rept 100000000\nnop\nnop\n.endr", "too many statements");
}

#[test]
fn conditionals() {
    let words = text(
        ".equ V, 2
         .if V == 2
           .word 1
         .elseif V == 3
           .word 2
         .else
           .word 3
         .endif
         .if 0
           .if 1
             .word 99
           .else
             .word 98
           .endif
           this line is not assembled @#!
         .elseif 1
           .word 4
         .elseif 1
           .word 97
         .else
           .word 96
         .endif
         .ifdef V
           .word 5
         .endif
         .ifndef NOPE
           .word 6
         .endif
         .ifc abc, abc
           .word 7
         .endif
         .ifnc abc, abd
           .word 8
         .endif
         .ifb
           .word 9
         .endif
         .ifeq V - 2
           .word 10
         .endif
         .ifne V
           .word 11
         .endif
         .iflt -1
           .word 12
         .endif
         .ifge 0
           .word 13
         .endif
         .ifgt 0
           .word 95
         .endif
         .ifle 0
           .word 14
         .endif",
    );
    let mut expect = vec![1];
    expect.extend(4..=14);
    assert_eq!(words, expect);
    assert_error(".if 1\nnop", "unterminated conditional");
    assert_error(".endif", ".endif without matching .if");
    assert_error(".else", ".else without matching .if");
    assert_error(".if 1\n.else\n.else\n.endif", "duplicate .else");
    assert_error(".if later\n.endif\nlater:", "known at this point");
}

#[test]
fn error_and_warning_directives() {
    assert_error(
        ".error \"custom failure\"",
        "t.s:1:1: error: custom failure",
    );
    let out = assemble_sources(
        &[("t.s", ".warning \"careful\"\nnop")],
        &Options {
            layout: LayoutSpec::Org(0),
            ..Options::default()
        },
    );
    assert!(out.program.is_some(), "warnings do not fail the build");
    assert!(out
        .diags
        .render(&out.files, "x")
        .contains("t.s:1:1: warning: careful"));
}

#[test]
fn ignored_directives_and_option_balance() {
    let words = text(
        ".file \"x.c\"
         .ident \"clang\"
         .attribute 5, \"rv32i2p1\"
         .option push
         .option norelax
         .option arch, +c
         .option pop
         .cfi_startproc
         .cfi_def_cfa_offset 16
         .cfi_endproc
         .addrsig
         .addrsig_sym foo
         .hidden foo
         .protected foo
         .internal foo
         nop",
    );
    assert_eq!(words, vec![NOP]);
    assert_error(".option pop", ".option pop without .option push");
    assert!(
        diags(".option push\nnop").contains("warning: .option push without matching .option pop")
    );
}

// ----- include / incbin / -D -------------------------------------------------------------------------

#[test]
fn include_incbin_and_defines() {
    let dir = common::scratch("asm_syntax_include");
    std::fs::create_dir_all(dir.join("inc")).unwrap();
    std::fs::write(
        dir.join("inc/defs.inc"),
        ".equ FROM_INC, 42\n.macro TWICE x\n.word \\x, \\x\n.endm\n",
    )
    .unwrap();
    std::fs::write(dir.join("blob.bin"), [1u8, 2, 3, 4, 5, 6]).unwrap();
    std::fs::write(dir.join("bad.inc"), "nop\n  bogus a0\n").unwrap();
    let main = dir.join("main.s");
    std::fs::write(
        &main,
        ".include \"defs.inc\"\n.data\n.word FROM_INC, DEF_A\nTWICE 3\n.incbin \"blob.bin\", 1, 3\n.incbin \"blob.bin\"\n.if DEF_B\n.byte 9\n.endif\n",
    )
    .unwrap();
    let opts = Options {
        inputs: vec![main.to_string_lossy().into_owned()],
        layout: LayoutSpec::Org(0),
        include_dirs: vec![dir.join("inc")],
        defines: vec![("DEF_A".into(), 0x55), ("DEF_B".into(), 1)],
    };
    let out = strata_asm::assemble_and_link(&opts);
    let p = out
        .program
        .unwrap_or_else(|| panic!("{}", out.diags.render(&out.files, "x")));
    let mut expect = vec![42, 0, 0, 0, 0x55, 0, 0, 0, 3, 0, 0, 0, 3, 0, 0, 0, 2, 3, 4];
    expect.extend([1, 2, 3, 4, 5, 6, 9]);
    assert_eq!(section_bytes(&p, ".data"), expect);
    // -D symbols are not output.
    assert!(p.symbol("DEF_A").is_none());

    // Errors inside an included file carry its name.
    std::fs::write(&main, ".include \"bad.inc\"\n").unwrap();
    let out = strata_asm::assemble_and_link(&Options {
        inputs: opts.inputs.clone(),
        ..Options::default()
    });
    let d = out.diags.render(&out.files, "x");
    assert!(
        d.contains("bad.inc:2:3: error: unknown instruction 'bogus'"),
        "{d}"
    );
    std::fs::write(&main, ".include \"missing.inc\"\n.incbin \"nope.bin\"\n").unwrap();
    let out = strata_asm::assemble_and_link(&Options {
        inputs: opts.inputs.clone(),
        ..Options::default()
    });
    let d = out.diags.render(&out.files, "x");
    assert!(
        d.contains("cannot find include file 'missing.inc'")
            && d.contains("cannot find file 'nope.bin'"),
        "{d}"
    );
    std::fs::write(&main, ".include \"main.s\"\n").unwrap();
    let out = strata_asm::assemble_and_link(&Options {
        inputs: opts.inputs,
        ..Options::default()
    });
    assert!(out
        .diags
        .render(&out.files, "x")
        .contains("includes nested too deeply"));
}

// ----- debug directives -----------------------------------------------------------------------------

#[test]
fn file_and_loc_build_line_table() {
    let src = ".file \"m.c\"
        .file 0 \"/work/strata\" \"sw/m.c\" md5 0x0123456789abcdef0123456789abcdef
        .file 1 \"sw/include\" \"h.h\"
        .file 2 \"/usr/include\" \"stdint.h\"
        .text
        f:
        .loc 0 10 3
        nop
        nop
        .loc 1 20 0 prologue_end
        .loc 0 11 7 is_stmt 0
        nop
        .loc 2 5 1 discriminator 3
        nop
        .loc 0 12 0 is_stmt 1
        nop
        .data
        .word 1";
    let out = assemble_sources(
        &[("m.s", src)],
        &Options {
            layout: LayoutSpec::Org(0x100),
            ..Options::default()
        },
    );
    let p = out.program.expect("assembles");
    let info = DebugInfo::from_program(&p, &out.files);
    assert_eq!(info.files[0], "m.s");
    let name = |i: u32| info.files[i as usize].as_str();
    let lines: Vec<(u32, &str, u32, u32, u8)> = info
        .lines
        .iter()
        .map(|l| (l.0, name(l.1), l.2, l.3, l.4))
        .collect();
    assert_eq!(
        lines,
        vec![
            (0x100, "sw/m.c", 10, 3, 1),
            (0x108, "sw/m.c", 11, 7, 0),
            // is_stmt is state: it stays 0 until a `.loc` sets it again.
            (0x10c, "/usr/include/stdint.h", 5, 1, 0),
            (0x110, "sw/m.c", 12, 0, 1),
            (0x114, "sw/m.c", 0, 0, 0),
        ]
    );
    assert!(info.files.iter().any(|f| f == "sw/m.c"));
    assert_eq!(info.asm_lines.len(), 5);
    assert_error(".loc 3 1 1\nnop", "file number 3 was not declared");
}

#[test]
fn no_panics_on_garbage() {
    let samples = [
        "\u{0}\u{1}\u{7f}",
        "((((((((",
        ")))",
        ".macro",
        ".irp",
        ".section ,,,",
        "%hi(",
        "lw a0, %lo(x",
        "lw a0, )(",
        "0x",
        "\"unterminated",
        "'",
        ".byte '",
        "li a0, 99999999999999999999999999",
        ".fill 1, 1, ",
        ":",
        "::",
        "1:",
        ".set",
        "= 5",
        ".type , ",
        ".size x",
        ".comm",
        ".loc",
        ".file 1",
        ".p2align",
        "fence r,",
        "call",
        "la a0,",
        "beq a0, a1,",
        "é ü 漢字",
        ".ascii \"\\x\"",
        ".include",
        ".incbin \"\", 99999999999",
        ".org -5",
        ".rept 99999999999\n.endr",
    ];
    for s in samples {
        let _ = diags(s);
    }
    // Resource limits: deep nesting, huge expressions, exponential macros and
    // `.set` chains, huge fills.
    let deep = format!(".word {}1{}", "(".repeat(50_000), ")".repeat(50_000));
    assert!(diags(&deep).contains("expression is too complex"));
    let long = format!(".data\n.word {}\na:", vec!["a"; 50_000].join("+"));
    assert!(diags(&long).contains("expression is too complex"));
    assert!(diags(".macro m\nm\nm\n.endm\nm").contains("expanded recursively too deeply"));
    let doubling = format!(
        ".set x, y + y\n{}.word x\n.equ y, 1\n",
        ".set x, x + x\n".repeat(60)
    );
    assert!(diags(&doubling).contains("error"));
    assert!(diags(".data\n.zero 0x7fffffff").contains("would exceed"));
    assert!(diags(".data\n.fill 0x7fffffff, 8, 1").contains("fill too large"));
    assert!(
        !diags(".bss\n.zero 0x7fffffff").contains("error"),
        "large .bss is fine"
    );
    // Random byte soup, line by line.
    let mut rng = common::Rng::new(77);
    let alphabet = b"abcxz019 ,.()%:;#\"'\\-+*/<>=!~&|^@{}\n\tlwa0sp";
    for _ in 0..300 {
        let n = rng.range(1, 60) as usize;
        let s: String = (0..n).map(|_| *rng.pick(alphabet) as char).collect();
        let _ = diags(&s);
    }
}
