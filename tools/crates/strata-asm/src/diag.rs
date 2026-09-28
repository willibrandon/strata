//! Source locations and diagnostics.

use std::collections::HashMap;
use std::fmt::Write as _;

/// Index of a file name in a [`FileTable`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct FileId(pub u32);

/// A position in (logical) source text. Lines and columns are 1-based; a
/// column of 0 means "whole line".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Loc {
    /// File (after line markers).
    pub file: FileId,
    /// 1-based line.
    pub line: u32,
    /// 1-based byte column; 0 if unknown.
    pub col: u32,
}

impl Loc {
    /// The same location with a different column.
    pub fn with_col(self, col: u32) -> Loc {
        Loc { col, ..self }
    }
}

/// Interned file names. Names are stored exactly as given (command line,
/// `.include`, line markers, `.file`).
#[derive(Debug, Default, Clone)]
pub struct FileTable {
    names: Vec<String>,
    index: HashMap<String, FileId>,
}

impl FileTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the id of `name`, adding it if needed.
    pub fn intern(&mut self, name: &str) -> FileId {
        if let Some(&id) = self.index.get(name) {
            return id;
        }
        let id = FileId(self.names.len() as u32);
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), id);
        id
    }

    /// The name of file `id`.
    pub fn name(&self, id: FileId) -> &str {
        self.names
            .get(id.0 as usize)
            .map_or("<unknown>", String::as_str)
    }
}

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Fails the build.
    Error,
    /// Reported only.
    Warning,
    /// Additional information attached to another diagnostic.
    Note,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

/// One message, optionally tied to a source location, with attached notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Error or warning.
    pub severity: Severity,
    /// Where, if tied to source.
    pub loc: Option<Loc>,
    /// The message.
    pub message: String,
    /// Attached notes.
    pub notes: Vec<(Option<Loc>, String)>,
}

/// Collected diagnostics of a run.
#[derive(Debug, Default, Clone)]
pub struct Diags {
    /// Diagnostics in the order they were reported.
    pub list: Vec<Diagnostic>,
    /// Whether the most recent report was recorded (notes attach only then).
    last_recorded: bool,
}

/// Errors beyond this count are dropped (a summary line is kept) so that a
/// wrong input file cannot produce unbounded output.
const MAX_ERRORS: usize = 200;

impl Diags {
    /// No diagnostics.
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, severity: Severity, loc: Option<Loc>, message: String) {
        self.last_recorded = false;
        // The same problem found twice (e.g. both halves of a `call`) is reported once.
        if self
            .list
            .iter()
            .any(|d| d.severity == severity && d.loc == loc && d.message == message)
        {
            return;
        }
        if severity == Severity::Error && self.error_count() == MAX_ERRORS {
            self.list.push(Diagnostic {
                severity,
                loc: None,
                message: "too many errors; further errors suppressed".to_string(),
                notes: Vec::new(),
            });
        }
        if severity == Severity::Error && self.error_count() > MAX_ERRORS {
            return;
        }
        self.list.push(Diagnostic {
            severity,
            loc,
            message,
            notes: Vec::new(),
        });
        self.last_recorded = true;
    }

    /// Records an error at `loc`.
    pub fn error(&mut self, loc: Loc, message: impl Into<String>) {
        self.push(Severity::Error, Some(loc), message.into());
    }

    /// Records an error without a source location.
    pub fn error_global(&mut self, message: impl Into<String>) {
        self.push(Severity::Error, None, message.into());
    }

    /// Records a warning at `loc`.
    pub fn warning(&mut self, loc: Loc, message: impl Into<String>) {
        self.push(Severity::Warning, Some(loc), message.into());
    }

    /// Records a warning without a source location.
    pub fn warning_global(&mut self, message: impl Into<String>) {
        self.push(Severity::Warning, None, message.into());
    }

    /// Attaches a note to the most recent diagnostic (if it was recorded).
    pub fn note(&mut self, loc: Option<Loc>, message: impl Into<String>) {
        if !self.last_recorded {
            return;
        }
        if let Some(last) = self.list.last_mut() {
            last.notes.push((loc, message.into()));
        }
    }

    /// Number of errors recorded.
    pub fn error_count(&self) -> usize {
        self.list
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    /// True if any error was recorded.
    pub fn has_errors(&self) -> bool {
        self.list.iter().any(|d| d.severity == Severity::Error)
    }

    /// Formats all diagnostics as `file:line:col: severity: message` lines.
    pub fn render(&self, files: &FileTable, program: &str) -> String {
        let mut out = String::new();
        let fmt_loc = |loc: Option<Loc>| match loc {
            Some(l) if l.col > 0 => format!("{}:{}:{}", files.name(l.file), l.line, l.col),
            Some(l) => format!("{}:{}", files.name(l.file), l.line),
            None => program.to_string(),
        };
        for d in &self.list {
            let _ = writeln!(
                out,
                "{}: {}: {}",
                fmt_loc(d.loc),
                d.severity.label(),
                d.message
            );
            for (loc, note) in &d.notes {
                let _ = writeln!(out, "{}: note: {}", fmt_loc(*loc), note);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_format() {
        let mut files = FileTable::new();
        let f = files.intern("a.s");
        assert_eq!(files.intern("a.s"), f);
        let mut d = Diags::new();
        d.error(
            Loc {
                file: f,
                line: 3,
                col: 7,
            },
            "bad thing",
        );
        d.note(
            Some(Loc {
                file: f,
                line: 1,
                col: 0,
            }),
            "defined here",
        );
        d.warning_global("careful");
        assert!(d.has_errors());
        assert_eq!(
            d.render(&files, "strata-as"),
            "a.s:3:7: error: bad thing\na.s:1: note: defined here\nstrata-as: warning: careful\n"
        );
    }

    #[test]
    fn error_cap() {
        let mut d = Diags::new();
        for i in 0..500 {
            d.error_global(format!("error {i}"));
        }
        assert_eq!(d.error_count(), MAX_ERRORS + 1);
        // Identical diagnostics are reported once.
        let mut d = Diags::new();
        d.error_global("same");
        d.error_global("same");
        assert_eq!(d.error_count(), 1);
    }
}
