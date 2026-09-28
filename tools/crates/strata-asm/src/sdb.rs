//! The `.sdb` debug-information format (JSON), written by `strata-as` and
//! read by `strata-pack` and the debugger.

use crate::diag::{FileId, FileTable};
use crate::link::Program;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// An output section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdbSection {
    /// Output section name.
    pub name: String,
    /// Start address.
    pub addr: u32,
    /// Size in bytes.
    pub size: u32,
    /// Subset of `"awx"`.
    pub flags: String,
    /// Holds no bytes in the image (`.bss`).
    pub nobits: bool,
}

/// A symbol. `kind` is `func`, `object`, `section` or `notype`; `bind` is
/// `global`, `local` or `weak`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdbSymbol {
    /// Symbol name.
    pub name: String,
    /// Address or absolute value.
    pub addr: u32,
    /// Size from `.size` (0 if none).
    pub size: u32,
    /// `func`, `object`, `section` or `notype`.
    pub kind: String,
    /// `global`, `local` or `weak`.
    pub bind: String,
    /// Output section name; `null` for absolute symbols.
    pub section: Option<String>,
    /// Index in `files` of the input file that defined the symbol; `null`
    /// for symbols defined by the layout.
    pub file: Option<u32>,
}

/// Whole debug-information document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugInfo {
    /// Always `"strata-debug"`.
    pub format: String,
    /// Format version (1).
    pub version: u32,
    /// Entry point address.
    pub entry: u32,
    /// Non-empty output sections.
    pub sections: Vec<SdbSection>,
    /// Symbols sorted by address (`.L*` and numeric local labels omitted).
    pub symbols: Vec<SdbSymbol>,
    /// Input files first (in command-line order), then files named by
    /// `.file` and line markers.
    pub files: Vec<String>,
    /// `[addr, file, line, column, is_stmt]`, sorted by address; each row
    /// applies up to the next row's address. Rows with `line == 0` end a
    /// sequence (the addresses that follow have no source line).
    pub lines: Vec<(u32, u32, u32, u32, u8)>,
    /// `[addr, file, line]`: assembler-input location of every instruction.
    pub asm_lines: Vec<(u32, u32, u32)>,
}

impl DebugInfo {
    /// Builds the debug information of a linked program.
    pub fn from_program(p: &Program, files: &FileTable) -> DebugInfo {
        let mut index: HashMap<FileId, u32> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        let intern = |id: FileId, index: &mut HashMap<FileId, u32>, names: &mut Vec<String>| {
            *index.entry(id).or_insert_with(|| {
                names.push(files.name(id).to_string());
                (names.len() - 1) as u32
            })
        };
        // Input files first, so that a symbol's `file` is its input's position.
        let mut input_index = Vec::new();
        for inp in &p.inputs {
            // Distinct inputs may share a path; keep one entry per input.
            names.push(inp.path.clone());
            let i = (names.len() - 1) as u32;
            index.entry(inp.file).or_insert(i);
            input_index.push(i);
        }
        let lines = p
            .lines
            .iter()
            .map(|l| {
                (
                    l.addr,
                    intern(l.file, &mut index, &mut names),
                    l.line,
                    l.col,
                    u8::from(l.is_stmt),
                )
            })
            .collect();
        let asm_lines = p
            .asm_lines
            .iter()
            .map(|a| (a.addr, intern(a.file, &mut index, &mut names), a.line))
            .collect();
        let sections = p
            .sections
            .iter()
            .filter(|s| s.size > 0)
            .map(|s| SdbSection {
                name: s.name.clone(),
                addr: s.addr,
                size: s.size,
                flags: s.flags.text(),
                nobits: s.nobits,
            })
            .collect();
        let symbols = p
            .symbols
            .iter()
            .map(|s| SdbSymbol {
                name: s.name.clone(),
                addr: s.addr,
                size: s.size,
                kind: s.kind.as_str().to_string(),
                bind: s.bind.as_str().to_string(),
                section: s.section.map(|i| p.sections[i].name.clone()),
                file: s.file.and_then(|f| input_index.get(f).copied()),
            })
            .collect();
        DebugInfo {
            format: "strata-debug".to_string(),
            version: 1,
            entry: p.entry,
            sections,
            symbols,
            files: names,
            lines,
            asm_lines,
        }
    }

    /// Serializes to JSON: one top-level key per line, compact values.
    pub fn to_json(&self) -> String {
        let field = |name: &str, v: serde_json::Result<String>| {
            format!("  \"{name}\": {}", v.unwrap_or_else(|_| "null".into()))
        };
        let rows = |items: Vec<String>| {
            if items.is_empty() {
                "[]".to_string()
            } else {
                format!("[\n    {}\n  ]", items.join(",\n    "))
            }
        };
        let many = |v: Vec<serde_json::Result<String>>| {
            rows(v.into_iter().map(|s| s.unwrap_or_default()).collect())
        };
        let parts = [
            field("format", serde_json::to_string(&self.format)),
            field("version", serde_json::to_string(&self.version)),
            field("entry", serde_json::to_string(&self.entry)),
            format!(
                "  \"sections\": {}",
                many(self.sections.iter().map(serde_json::to_string).collect())
            ),
            format!(
                "  \"symbols\": {}",
                many(self.symbols.iter().map(serde_json::to_string).collect())
            ),
            format!(
                "  \"files\": {}",
                many(self.files.iter().map(serde_json::to_string).collect())
            ),
            format!(
                "  \"lines\": {}",
                many(self.lines.iter().map(serde_json::to_string).collect())
            ),
            format!(
                "  \"asm_lines\": {}",
                many(self.asm_lines.iter().map(serde_json::to_string).collect())
            ),
        ];
        format!("{{\n{}\n}}\n", parts.join(",\n"))
    }

    /// Parses `.sdb` JSON.
    pub fn from_json(text: &str) -> Result<DebugInfo, String> {
        let d: DebugInfo = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if d.format != "strata-debug" {
            return Err(format!("not a strata debug file (format '{}')", d.format));
        }
        if d.version != 1 {
            return Err(format!("unsupported debug file version {}", d.version));
        }
        Ok(d)
    }

    /// Address of symbol `name`, preferring a global definition; errors if
    /// the name is ambiguous among local symbols.
    pub fn symbol_addr(&self, name: &str) -> Result<u32, String> {
        let all: Vec<&SdbSymbol> = self.symbols.iter().filter(|s| s.name == name).collect();
        if let Some(g) = all.iter().find(|s| s.bind != "local") {
            return Ok(g.addr);
        }
        match all.as_slice() {
            [s] => Ok(s.addr),
            [] => Err(format!("symbol '{name}' not found")),
            _ => Err(format!(
                "symbol '{name}' is ambiguous ({} local definitions)",
                all.len()
            )),
        }
    }
}
