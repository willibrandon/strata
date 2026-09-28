//! In-memory objects produced by assembling one input file: sections with
//! their bytes and pending fixups, symbols, and per-instruction records used
//! for debug information and listings.

use crate::diag::{FileId, Loc};
use crate::expr::{Expr, SymId};
use std::collections::HashMap;

/// Section flags that are reflected in ELF output (`a w x M S`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SecFlags {
    pub alloc: bool,
    pub write: bool,
    pub exec: bool,
    pub merge: bool,
    pub strings: bool,
}

impl SecFlags {
    /// Parses a GNU flag string (`"aMS"`); unknown letters are ignored.
    pub fn parse(s: &str) -> SecFlags {
        let mut f = SecFlags::default();
        for c in s.chars() {
            match c {
                'a' => f.alloc = true,
                'w' => f.write = true,
                'x' => f.exec = true,
                'M' => f.merge = true,
                'S' => f.strings = true,
                _ => {}
            }
        }
        f
    }

    /// Default flags for a section created by name only.
    pub fn for_name(name: &str) -> SecFlags {
        let has = |p: &str| name == p || name.starts_with(&format!("{p}."));
        if has(".text") {
            SecFlags::parse("ax")
        } else if has(".data")
            || has(".sdata")
            || has(".bss")
            || has(".sbss")
            || has(".init_array")
            || has(".fini_array")
            || has(".preinit_array")
        {
            SecFlags::parse("aw")
        } else if has(".rodata") || has(".srodata") {
            SecFlags::parse("a")
        } else {
            SecFlags::default()
        }
    }

    /// Union of two flag sets.
    pub fn union(self, o: SecFlags) -> SecFlags {
        SecFlags {
            alloc: self.alloc || o.alloc,
            write: self.write || o.write,
            exec: self.exec || o.exec,
            merge: self.merge || o.merge,
            strings: self.strings || o.strings,
        }
    }

    /// `"awx"`-style text of the allocation flags.
    pub fn text(self) -> String {
        let mut s = String::new();
        if self.alloc {
            s.push('a');
        }
        if self.write {
            s.push('w');
        }
        if self.exec {
            s.push('x');
        }
        s
    }
}

/// How a fixup patches the bytes at its offset once the expression is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixKind {
    /// Little-endian data of 1, 2, 4 or 8 bytes. Only 4-byte data may be
    /// relocatable (an absolute 32-bit relocation).
    Data(u8),
    /// Bare immediate of an I-type instruction (must be absolute).
    ImmI,
    /// Bare immediate of an S-type instruction (must be absolute).
    ImmS,
    /// Shift amount (must be absolute, 0..=31).
    Shamt,
    /// Bare 20-bit immediate of `lui`/`auipc` (must be absolute).
    Imm20,
    /// `%hi(expr)` in `lui`.
    Hi20,
    /// `%lo(expr)` in an I-type immediate.
    Lo12I,
    /// `%lo(expr)` in an S-type immediate.
    Lo12S,
    /// `%pcrel_hi(expr)` in `auipc`.
    PcrelHi20,
    /// `%pcrel_lo` in an I-type immediate. `Some(off)`: the paired `auipc` is
    /// at `off` in the same section (pseudo-instruction expansions);
    /// `None`: the fixup expression is the label of the `auipc`.
    PcrelLo12I(Option<u64>),
    /// `%pcrel_lo` in an S-type immediate; see [`FixKind::PcrelLo12I`].
    PcrelLo12S(Option<u64>),
    /// Conditional branch target.
    Branch,
    /// `jal` target.
    Jal,
}

/// A value to be patched into a section once symbols are placed.
#[derive(Clone, Debug)]
pub struct Fixup {
    pub off: u64,
    pub kind: FixKind,
    pub expr: Expr,
    pub loc: Loc,
}

/// A `.loc` line-table row attached to the instruction at `off`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineRow {
    pub off: u64,
    pub file: FileId,
    pub line: u32,
    pub col: u32,
    pub is_stmt: bool,
}

/// A section of one object.
#[derive(Clone, Debug, Default)]
pub struct Section {
    pub name: String,
    pub flags: SecFlags,
    /// Holds no file bytes (`@nobits`, `.bss*`, `.sbss*`).
    pub nobits: bool,
    pub entsize: u64,
    /// Contents (empty for nobits sections).
    pub data: Vec<u8>,
    /// Size in bytes (equals `data.len()` unless nobits).
    pub size: u64,
    /// Required alignment (power of two, at least 1).
    pub align: u64,
    /// True once an instruction has been emitted.
    pub has_insns: bool,
    /// Contents and label references are dropped (`.debug*`, `.comment`, ...).
    pub discarded: bool,
    pub fixups: Vec<Fixup>,
    /// Alignment padding (offset, length) to be filled with `nop`s if the
    /// section turns out to contain code.
    pub code_pads: Vec<(u64, u64)>,
    /// Offset and source location of every emitted instruction.
    pub insns: Vec<(u64, Loc)>,
    /// `.loc` rows.
    pub lines: Vec<LineRow>,
    /// Location of the directive that created the section.
    pub loc: Loc,
}

impl Section {
    pub fn new(name: &str, flags: SecFlags, nobits: bool, loc: Loc) -> Section {
        Section {
            name: name.to_string(),
            flags,
            nobits,
            align: 1,
            loc,
            ..Default::default()
        }
    }
}

/// Symbol binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bind {
    Local,
    Global,
    Weak,
}

impl Bind {
    pub fn as_str(self) -> &'static str {
        match self {
            Bind::Local => "local",
            Bind::Global => "global",
            Bind::Weak => "weak",
        }
    }
}

/// Symbol type from `.type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymKind {
    NoType,
    Func,
    Object,
    Section,
}

impl SymKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymKind::NoType => "notype",
            SymKind::Func => "func",
            SymKind::Object => "object",
            SymKind::Section => "section",
        }
    }
}

/// What a symbol is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymDef {
    /// Referenced or declared but not defined in this object.
    Undef,
    /// A label: offset in a section of this object.
    Label { sec: u32, off: u64 },
    /// Defined by `.set`/`.equ`/`=`.
    Equ(Expr),
    /// Global common symbol (`.comm` without `.local`).
    Common { size: u64, align: u64 },
}

/// A symbol of one object.
#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub def: SymDef,
    pub bind: Bind,
    pub kind: SymKind,
    /// `.size` expression, evaluated at link time.
    pub size: Option<(Expr, Loc)>,
    pub def_loc: Option<Loc>,
    /// First reference, for "undefined symbol" diagnostics.
    pub ref_loc: Option<Loc>,
    /// Assembler-internal (`.L*`, numeric local labels): never output.
    pub temp: bool,
    /// Defined with `-D`: never output.
    pub cmdline: bool,
    /// Named by `.local` (a following `.comm` allocates in this file's `.bss`).
    pub local_directive: bool,
    /// For `x = y [+/- n]`: the symbol `y`, whose type an untyped `x` inherits.
    pub alias: Option<SymId>,
}

impl Symbol {
    pub fn new(name: &str) -> Symbol {
        let temp = name.starts_with(".L") || name.contains('\u{2}');
        Symbol {
            name: name.to_string(),
            def: SymDef::Undef,
            bind: Bind::Local,
            kind: SymKind::NoType,
            size: None,
            def_loc: None,
            ref_loc: None,
            temp,
            cmdline: false,
            local_directive: false,
            alias: None,
        }
    }

    /// True if the symbol has any definition in this object.
    pub fn is_defined(&self) -> bool {
        self.def != SymDef::Undef
    }

    /// Name for diagnostics (numeric local labels shown as `N`).
    pub fn display_name(&self) -> String {
        match self.name.split_once('\u{2}') {
            Some((n, _)) => n.to_string(),
            None => self.name.clone(),
        }
    }
}

/// One statement of the input, for listings.
#[derive(Clone, Debug)]
pub struct ListLine {
    pub loc: Loc,
    pub text: String,
    /// Section, start offset and length of the bytes the statement produced.
    pub place: Option<(u32, u64, u64)>,
    /// True if the bytes are instructions.
    pub code: bool,
}

/// The result of assembling one input file.
#[derive(Clone, Debug, Default)]
pub struct Object {
    /// Path as given on the command line (or a synthetic name).
    pub path: String,
    pub file: FileId,
    pub sections: Vec<Section>,
    pub symbols: Vec<Symbol>,
    pub sym_index: HashMap<String, SymId>,
    pub listing: Vec<ListLine>,
}

impl Object {
    pub fn new(path: &str, file: FileId) -> Object {
        Object {
            path: path.to_string(),
            file,
            ..Default::default()
        }
    }

    /// Returns the id of symbol `name`, creating an undefined one if needed.
    pub fn symbol_id(&mut self, name: &str) -> SymId {
        if let Some(&id) = self.sym_index.get(name) {
            return id;
        }
        let id = self.symbols.len() as SymId;
        self.symbols.push(Symbol::new(name));
        self.sym_index.insert(name.to_string(), id);
        id
    }

    /// Looks up a symbol without creating it.
    pub fn find_symbol(&self, name: &str) -> Option<SymId> {
        self.sym_index.get(name).copied()
    }

    pub fn find_section(&self, name: &str) -> Option<u32> {
        self.sections
            .iter()
            .position(|s| s.name == name)
            .map(|i| i as u32)
    }
}

/// True if a section of this name is dropped (debug info and metadata).
pub fn is_discarded_name(name: &str) -> bool {
    [".debug", ".comment", ".note", ".llvm", ".riscv.attributes"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// True if a section of this name holds no file bytes regardless of type.
pub fn is_nobits_name(name: &str) -> bool {
    let has = |p: &str| name == p || name.starts_with(&format!("{p}."));
    has(".bss") || has(".sbss")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags() {
        assert_eq!(SecFlags::parse("aMS").text(), "a");
        assert!(SecFlags::parse("aMS").merge);
        assert_eq!(SecFlags::for_name(".text.foo").text(), "ax");
        assert_eq!(SecFlags::for_name(".textual").text(), "");
        assert_eq!(SecFlags::for_name(".sbss").text(), "aw");
        assert_eq!(SecFlags::for_name(".rodata.str1.1").text(), "a");
        assert_eq!(SecFlags::for_name(".vectors").text(), "");
    }

    #[test]
    fn names() {
        assert!(is_discarded_name(".debug_info"));
        assert!(is_discarded_name(".note.GNU-stack"));
        assert!(is_discarded_name(".llvm_addrsig"));
        assert!(is_discarded_name(".riscv.attributes"));
        assert!(!is_discarded_name(".data"));
        assert!(is_nobits_name(".bss"));
        assert!(is_nobits_name(".sbss.x"));
        assert!(!is_nobits_name(".bssx"));
        assert!(Symbol::new(".Ltmp0").temp);
        assert!(Symbol::new("1\u{2}3").temp);
        assert_eq!(Symbol::new("1\u{2}3").display_name(), "1");
        assert!(!Symbol::new("main").temp);
    }
}
