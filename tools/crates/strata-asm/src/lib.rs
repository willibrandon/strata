//! `strata-as`, Strata's assembler and linker, and `strata-pack`, its boot
//! image builder (`docs/toolchain.md`).
//!
//! The library assembles every input file into an in-memory [`object::Object`]
//! ([`asm`]), links them according to a [`layout::Layout`] ([`link`]), and
//! writes the outputs ([`output`], [`sdb`]). [`pack`] implements the boot
//! image format. The command-line front ends live in [`cli`].
//!
//! ```no_run
//! use strata_asm::{assemble_and_link, LayoutSpec, Options};
//! let opts = Options {
//!     inputs: vec!["start.s".into(), "main.s".into()],
//!     layout: LayoutSpec::Org(0x1000_0000),
//!     ..Options::default()
//! };
//! let out = assemble_and_link(&opts);
//! eprint!("{}", out.diags.render(&out.files, "strata-as"));
//! let program = out.program.expect("no errors");
//! let (_base, bytes) = strata_asm::output::flat_binary(&program).unwrap();
//! # let _ = bytes;
//! ```

pub mod asm;
pub mod cli;
pub mod diag;
pub mod expr;
pub mod layout;
pub mod lexer;
pub mod link;
pub mod object;
pub mod output;
pub mod pack;
pub mod sdb;
pub mod source;

use diag::{Diags, FileTable};
use layout::Layout;
use link::Program;
use std::path::PathBuf;

/// Where the layout comes from.
#[derive(Clone, Debug)]
pub enum LayoutSpec {
    /// A layout file (`-T`).
    File(String),
    /// `--org ADDR`: everything contiguous from `ADDR`.
    Org(u32),
    /// An already parsed layout.
    Parsed(Layout),
}

impl Default for LayoutSpec {
    fn default() -> Self {
        LayoutSpec::Org(0)
    }
}

/// Inputs and settings of one assembler/linker run.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Input assembly files, in link order.
    pub inputs: Vec<String>,
    /// Layout file or `--org` address.
    pub layout: LayoutSpec,
    /// `-I` directories.
    pub include_dirs: Vec<PathBuf>,
    /// `-D` symbols.
    pub defines: Vec<(String, i64)>,
}

/// Result of a run: the file-name table (for rendering diagnostics), all
/// diagnostics, and the program if there were no errors.
#[derive(Debug)]
pub struct Outcome {
    /// Names of all files seen (for rendering diagnostics and debug info).
    pub files: FileTable,
    /// Errors and warnings.
    pub diags: Diags,
    /// The linked program, if there were no errors.
    pub program: Option<Program>,
}

fn load_layout(spec: &LayoutSpec, files: &mut FileTable, diags: &mut Diags) -> Option<Layout> {
    match spec {
        LayoutSpec::Org(a) => Some(Layout::org(*a)),
        LayoutSpec::Parsed(l) => Some(l.clone()),
        LayoutSpec::File(path) => {
            let id = files.intern(path);
            match std::fs::read_to_string(path) {
                Ok(text) => layout::parse_layout(&text, id, diags),
                Err(e) => {
                    diags.error_global(format!("cannot read layout file '{path}': {e}"));
                    None
                }
            }
        }
    }
}

/// Assembles and links the input files named in `opts`.
pub fn assemble_and_link(opts: &Options) -> Outcome {
    let mut files = FileTable::new();
    let mut diags = Diags::new();
    let cfg = asm::AsmConfig {
        include_dirs: opts.include_dirs.clone(),
        defines: opts.defines.clone(),
    };
    let layout = load_layout(&opts.layout, &mut files, &mut diags);
    let objects: Vec<_> = opts
        .inputs
        .iter()
        .enumerate()
        .map(|(i, path)| asm::assemble_file(i as u32, path, &mut files, &mut diags, &cfg))
        .collect();
    finish(objects, layout, opts, files, diags)
}

/// Like [`assemble_and_link`], with sources given as `(name, text)` pairs
/// instead of files (`.include` still reads files, relative to the working
/// directory and `-I` directories).
pub fn assemble_sources(sources: &[(&str, &str)], opts: &Options) -> Outcome {
    let mut files = FileTable::new();
    let mut diags = Diags::new();
    let cfg = asm::AsmConfig {
        include_dirs: opts.include_dirs.clone(),
        defines: opts.defines.clone(),
    };
    let layout = load_layout(&opts.layout, &mut files, &mut diags);
    let objects: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(i, (name, text))| {
            asm::assemble_text(
                i as u32,
                name,
                text,
                PathBuf::new(),
                &mut files,
                &mut diags,
                &cfg,
            )
        })
        .collect();
    finish(objects, layout, opts, files, diags)
}

fn finish(
    objects: Vec<object::Object>,
    layout: Option<Layout>,
    opts: &Options,
    files: FileTable,
    mut diags: Diags,
) -> Outcome {
    let program = match layout {
        Some(l) if !diags.has_errors() => link::link(objects, &l, &opts.defines, &mut diags),
        _ => None,
    };
    Outcome {
        files,
        diags,
        program,
    }
}
