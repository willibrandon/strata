//! Debug information from strata-as (`.sdb`, docs/toolchain.md) for every
//! program image the machine can execute: the boot ROM, the kernel, and the
//! user programs, several of which may share a slot. Maps addresses to
//! source and assembly lines and symbols, and source lines to addresses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SdbSymbol {
    pub name: String,
    pub addr: u32,
    pub size: u32,
    pub kind: String,
    pub bind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SdbSection {
    pub name: String,
    pub addr: u32,
    pub size: u32,
    pub nobits: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct SdbFile {
    entry: u32,
    sections: Vec<SdbSection>,
    symbols: Vec<SdbSymbol>,
    files: Vec<String>,
    lines: Vec<(u32, u32, u32, u32, u32)>,
    asm_lines: Vec<(u32, u32, u32)>,
}

/// A source location.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Loc {
    pub file: PathBuf,
    pub line: u32,
}

/// One linked program with its debug information.
#[derive(Debug, Clone)]
pub struct Image {
    pub name: String,
    pub lo: u32,
    pub hi: u32,
    pub entry: u32,
    pub sections: Vec<SdbSection>,
    pub symbols: Vec<SdbSymbol>,
    files: Vec<PathBuf>,
    /// (addr, file, line) sorted by address; line 0 ends a sequence.
    lines: Vec<(u32, u32, u32)>,
    asm_lines: Vec<(u32, u32, u32)>,
}

impl Image {
    pub fn load(name: &str, sdb: &Path, source_root: &Path) -> Result<Image, String> {
        let text = std::fs::read_to_string(sdb).map_err(|e| format!("{}: {e}", sdb.display()))?;
        let f: SdbFile = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", sdb.display()))?;
        let files = f
            .files
            .iter()
            .map(|p| {
                let p = PathBuf::from(p);
                if p.is_absolute() { p } else { source_root.join(p) }
            })
            .collect();
        let lo = f.sections.iter().filter(|s| s.size > 0).map(|s| s.addr).min().unwrap_or(0);
        let hi = f.sections.iter().map(|s| s.addr + s.size).max().unwrap_or(0);
        let mut lines: Vec<(u32, u32, u32)> = f.lines.iter().map(|r| (r.0, r.1, r.2)).collect();
        lines.sort_by_key(|r| r.0);
        let mut asm_lines = f.asm_lines.clone();
        asm_lines.sort_by_key(|r| r.0);
        let mut symbols = f.symbols;
        symbols.sort_by_key(|s| s.addr);
        Ok(Image { name: name.to_string(), lo, hi, entry: f.entry, sections: f.sections, symbols, files, lines, asm_lines })
    }

    pub fn contains(&self, addr: u32) -> bool {
        addr >= self.lo && addr < self.hi
    }

    /// Source line (from `.loc`) of the instruction at `addr`.
    pub fn source_line(&self, addr: u32) -> Option<Loc> {
        let i = self.lines.partition_point(|r| r.0 <= addr);
        let r = self.lines.get(i.checked_sub(1)?)?;
        (r.2 != 0).then(|| Loc { file: self.files[r.1 as usize].clone(), line: r.2 })
    }

    /// Assembly line (the assembler's input) of the instruction at `addr`.
    pub fn asm_line(&self, addr: u32) -> Option<Loc> {
        let i = self.asm_lines.partition_point(|r| r.0 <= addr);
        let r = self.asm_lines.get(i.checked_sub(1)?)?;
        (r.0 == addr).then(|| Loc { file: self.files[r.1 as usize].clone(), line: r.2 })
    }

    /// Addresses whose source line is `loc` (first address of each run).
    pub fn addrs_of_line(&self, loc: &Loc) -> Vec<u32> {
        let Some(fi) = self.files.iter().position(|f| f == &loc.file) else { return Vec::new() };
        let mut out = Vec::new();
        let mut prev_match = false;
        for r in &self.lines {
            let m = r.1 as usize == fi && r.2 == loc.line;
            if m && !prev_match {
                out.push(r.0);
            }
            prev_match = m;
        }
        out
    }

    /// The function or object containing `addr`, with the offset into it.
    pub fn symbol_at(&self, addr: u32) -> Option<(&SdbSymbol, u32)> {
        let i = self.symbols.partition_point(|s| s.addr <= addr);
        self.symbols[..i]
            .iter()
            .rev()
            .find(|s| (s.size == 0 && s.kind != "object") || addr < s.addr + s.size)
            .filter(|s| !s.name.starts_with(".L"))
            .map(|s| (s, addr - s.addr))
    }

    pub fn symbol(&self, name: &str) -> Option<&SdbSymbol> {
        self.symbols.iter().find(|s| s.name == name)
    }

    /// Source files that contribute code.
    pub fn source_files(&self) -> Vec<PathBuf> {
        let mut used: Vec<bool> = vec![false; self.files.len()];
        for r in &self.lines {
            if r.2 != 0 {
                used[r.1 as usize] = true;
            }
        }
        let mut v: Vec<PathBuf> =
            self.files.iter().zip(used).filter(|(_, u)| *u).map(|(f, _)| f.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}

/// All program images, and the rule for which one is live at an address.
#[derive(Debug, Default)]
pub struct DebugInfo {
    pub images: Vec<Image>,
    sources: HashMap<PathBuf, Vec<String>>,
}

impl DebugInfo {
    pub fn add(&mut self, img: Image) {
        self.images.push(img);
    }

    /// Images covering `addr`. Several user programs can share a slot; the
    /// caller picks the one whose name matches the slot's program header.
    pub fn images_at(&self, addr: u32) -> impl Iterator<Item = &Image> {
        self.images.iter().filter(move |i| i.contains(addr))
    }

    /// The image for `addr`, preferring the program named `slot_program`.
    pub fn image_at(&self, addr: u32, slot_program: Option<&str>) -> Option<&Image> {
        let mut best = None;
        for img in self.images_at(addr) {
            if Some(img.name.as_str()) == slot_program {
                return Some(img);
            }
            best.get_or_insert(img);
        }
        best
    }

    /// The lines of a source file (cached).
    pub fn source(&mut self, path: &Path) -> Option<&[String]> {
        if !self.sources.contains_key(path) {
            let text = std::fs::read_to_string(path).ok()?;
            self.sources.insert(path.to_path_buf(), text.lines().map(str::to_string).collect());
        }
        self.sources.get(path).map(Vec::as_slice)
    }
}
