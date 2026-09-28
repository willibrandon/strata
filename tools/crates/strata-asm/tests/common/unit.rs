//! Helpers for the unit-style integration tests (no external tools).

#![allow(dead_code)]

use strata_asm::layout::{parse_layout, Layout};
use strata_asm::link::Program;
use strata_asm::{assemble_sources, LayoutSpec, Options};

/// Assembles one source at org 0 and returns the program (panics on error).
pub fn prog(src: &str) -> Program {
    prog_at(src, 0)
}

/// Assembles one source at `org`.
pub fn prog_at(src: &str, org: u32) -> Program {
    progs(&[("t.s", src)], LayoutSpec::Org(org))
}

/// Assembles several sources with a layout.
pub fn progs(sources: &[(&str, &str)], layout: LayoutSpec) -> Program {
    let out = assemble_sources(
        sources,
        &Options {
            layout,
            ..Options::default()
        },
    );
    match out.program {
        Some(p) => p,
        None => panic!(
            "unexpected errors:\n{}",
            out.diags.render(&out.files, "strata-as")
        ),
    }
}

/// Words of `.text` of a single source at org 0.
pub fn text(src: &str) -> Vec<u32> {
    section_words(&prog(src), ".text")
}

pub fn section_words(p: &Program, name: &str) -> Vec<u32> {
    let s = p.sections.iter().find(|s| s.name == name).expect("section");
    s.data
        .chunks(4)
        .map(|c| {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            u32::from_le_bytes(w)
        })
        .collect()
}

pub fn section_bytes(p: &Program, name: &str) -> Vec<u8> {
    p.sections
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.data.clone())
        .unwrap_or_default()
}

/// Rendered diagnostics (errors and warnings) of assembling `src` as `t.s`.
pub fn diags(src: &str) -> String {
    diags_with(&[("t.s", src)], LayoutSpec::Org(0))
}

pub fn diags_with(sources: &[(&str, &str)], layout: LayoutSpec) -> String {
    let out = assemble_sources(
        sources,
        &Options {
            layout,
            ..Options::default()
        },
    );
    out.diags.render(&out.files, "strata-as")
}

/// Asserts that assembling fails with a diagnostic containing `needle`.
#[track_caller]
pub fn assert_error(src: &str, needle: &str) {
    let out = assemble_sources(
        &[("t.s", src)],
        &Options {
            layout: LayoutSpec::Org(0),
            ..Options::default()
        },
    );
    let text = out.diags.render(&out.files, "strata-as");
    assert!(
        out.program.is_none(),
        "expected failure for {src:?}, got success; diagnostics:\n{text}"
    );
    assert!(
        text.contains(needle),
        "expected {needle:?} in diagnostics of {src:?}:\n{text}"
    );
}

/// Parses a layout (panics on error).
pub fn layout(text: &str) -> Layout {
    let mut d = strata_asm::diag::Diags::new();
    parse_layout(text, strata_asm::diag::FileId(0), &mut d).expect("layout parses")
}

/// Address of a symbol.
pub fn sym(p: &Program, name: &str) -> u32 {
    p.symbol(name)
        .unwrap_or_else(|| panic!("symbol {name} not found"))
        .addr
}
