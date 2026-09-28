//! Layout files: memory regions, output sections, symbol definitions, entry.
//!
//! ```text
//! memory RAM  origin=0x10000000 length=16K
//! entry _start
//! output .text   in RAM { .text.start .text .text.* }
//! output .bss    in RAM align=4 nobits { .bss .bss.* COMMON }
//! define __stack_top = ORIGIN(RAM) + LENGTH(RAM)
//! ```

use crate::diag::{Diags, FileId, Loc};

/// A memory region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    /// Region name.
    pub name: String,
    /// First address.
    pub origin: u64,
    /// Size in bytes.
    pub length: u64,
    /// Where it was declared.
    pub loc: Loc,
}

/// An `output` rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputRule {
    /// Output section name.
    pub name: String,
    /// Index into [`Layout::regions`].
    pub region: usize,
    /// Fixed start address (`at=`).
    pub at: Option<u64>,
    /// Minimum alignment (`align=`, default 1).
    pub align: u64,
    /// Holds no file bytes (`nobits`).
    pub nobits: bool,
    /// Patterns over input section names, in placement order.
    pub patterns: Vec<Pattern>,
    /// Where it was declared.
    pub loc: Loc,
}

/// One pattern of an output rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    /// Glob over input section names (`*`, `?`); `COMMON` selects common symbols.
    pub glob: String,
    /// Written as `KEEP(glob)`: matching input sections are roots of
    /// `--gc-sections` (they match and are placed like any other pattern).
    pub keep: bool,
}

impl Pattern {
    /// An ordinary (non-`KEEP`) pattern.
    pub fn new(glob: &str) -> Pattern {
        Pattern {
            glob: glob.to_string(),
            keep: false,
        }
    }
}

/// Expression of a `define` statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LExpr {
    /// A number.
    Num(i64),
    /// A symbol (program, layout or `-D`).
    Sym(String),
    /// `START`, `END`, `SIZE` (of an output section), `ORIGIN`, `LENGTH` (of a region).
    Query(String, String),
    /// `ALIGN(value, n)`.
    Align(Box<LExpr>, Box<LExpr>),
    /// Unary minus.
    Neg(Box<LExpr>),
    /// Binary operator `+ - * / & |`.
    Bin(char, Box<LExpr>, Box<LExpr>),
}

/// A `define` statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Define {
    /// Symbol defined.
    pub name: String,
    /// Its value.
    pub expr: LExpr,
    /// Where it was declared.
    pub loc: Loc,
}

/// A parsed layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// Memory regions in declaration order.
    pub regions: Vec<Region>,
    /// Output sections in placement order.
    pub outputs: Vec<OutputRule>,
    /// Symbol definitions, evaluated in order after placement.
    pub defines: Vec<Define>,
    /// Entry symbol.
    pub entry: Option<(String, Loc)>,
}

impl Layout {
    /// The layout used by `--org ADDR`: `.text*`, `.rodata*`, `.data*`, `.bss*`
    /// and `COMMON`, contiguous from `origin`. Small-data sections
    /// (`.srodata*`, `.sdata*`, `.sbss*`) join their large counterparts.
    pub fn org(origin: u32) -> Layout {
        let loc = Loc::default();
        let rule = |name: &str, nobits: bool, pats: &[&str]| OutputRule {
            name: name.to_string(),
            region: 0,
            at: None,
            align: 1,
            nobits,
            patterns: pats.iter().map(|s| Pattern::new(s)).collect(),
            loc,
        };
        Layout {
            regions: vec![Region {
                name: "ORG".to_string(),
                origin: u64::from(origin),
                length: (1u64 << 32) - u64::from(origin),
                loc,
            }],
            outputs: vec![
                rule(".text", false, &[".text", ".text.*"]),
                rule(
                    ".rodata",
                    false,
                    &[".rodata", ".rodata.*", ".srodata", ".srodata.*"],
                ),
                rule(".data", false, &[".data", ".data.*", ".sdata", ".sdata.*"]),
                rule(
                    ".bss",
                    true,
                    &[".bss", ".bss.*", ".sbss", ".sbss.*", "COMMON"],
                ),
            ],
            defines: Vec::new(),
            entry: None,
        }
    }

    pub fn region_index(&self, name: &str) -> Option<usize> {
        self.regions.iter().position(|r| r.name == name)
    }
}

/// Matches `name` against a glob pattern with `*` and `?`.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.as_bytes(), name.as_bytes());
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

struct Cursor<'s> {
    s: &'s [u8],
    pos: usize,
    line: u32,
    line_start: usize,
    file: FileId,
    depth: u32,
}

type PResult<T> = Result<T, (Loc, String)>;

impl<'s> Cursor<'s> {
    fn loc(&self) -> Loc {
        Loc {
            file: self.file,
            line: self.line,
            col: (self.pos - self.line_start + 1) as u32,
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> {
        Err((self.loc(), msg.into()))
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    /// Skips blanks and comments; newlines too if `newlines`.
    fn skip(&mut self, newlines: bool) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'\n' if newlines => {
                    self.pos += 1;
                    self.line += 1;
                    self.line_start = self.pos;
                }
                b'#' => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn at_line_end(&mut self) -> bool {
        self.skip(false);
        matches!(self.peek(), None | Some(b'\n'))
    }

    fn word(&mut self) -> Option<String> {
        self.skip(false);
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || b"_.$".contains(&c))
        {
            self.pos += 1;
        }
        (self.pos > start).then(|| String::from_utf8_lossy(&self.s[start..self.pos]).into_owned())
    }

    fn expect_word(&mut self, what: &str) -> PResult<String> {
        match self.word() {
            Some(w) => Ok(w),
            None => self.err(format!("expected {what}")),
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip(false);
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8) -> PResult<()> {
        if self.eat(c) {
            Ok(())
        } else {
            self.err(format!("expected '{}'", c as char))
        }
    }

    fn number(&mut self) -> PResult<i64> {
        self.skip(false);
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
            self.pos += 1;
        }
        let text = String::from_utf8_lossy(&self.s[start..self.pos]).into_owned();
        parse_number(&text).ok_or_else(|| {
            (
                Loc {
                    col: (start - self.line_start + 1) as u32,
                    ..self.loc()
                },
                format!("invalid number '{text}'"),
            )
        })
    }

    // expr   := or
    // or     := and ('|' and)*
    // and    := sum ('&' sum)*
    // sum    := prod (('+'|'-') prod)*
    // prod   := unary (('*'|'/') unary)*
    fn expr(&mut self) -> PResult<LExpr> {
        self.bin_level(0)
    }

    fn bin_level(&mut self, level: usize) -> PResult<LExpr> {
        const LEVELS: [&[u8]; 4] = [b"|", b"&", b"+-", b"*/"];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.bin_level(level + 1)?;
        loop {
            self.skip(false);
            match self.peek() {
                Some(c) if LEVELS[level].contains(&c) => {
                    self.pos += 1;
                    let rhs = self.bin_level(level + 1)?;
                    lhs = LExpr::Bin(c as char, Box::new(lhs), Box::new(rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn unary(&mut self) -> PResult<LExpr> {
        self.depth += 1;
        if self.depth > 256 {
            return self.err("expression is nested too deeply");
        }
        let e = self.unary_inner();
        self.depth -= 1;
        e
    }

    fn unary_inner(&mut self) -> PResult<LExpr> {
        self.skip(false);
        match self.peek() {
            Some(b'-') => {
                self.pos += 1;
                Ok(LExpr::Neg(Box::new(self.unary()?)))
            }
            Some(b'(') => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(b')')?;
                Ok(e)
            }
            Some(c) if c.is_ascii_digit() => Ok(LExpr::Num(self.number()?)),
            Some(_) => {
                let Some(w) = self.word() else {
                    return self.err("expected an expression");
                };
                let upper = w.to_ascii_uppercase();
                match upper.as_str() {
                    "START" | "END" | "SIZE" | "ORIGIN" | "LENGTH" if self.eat(b'(') => {
                        let arg = self.expect_word("a section or region name")?;
                        self.expect(b')')?;
                        Ok(LExpr::Query(upper, arg))
                    }
                    "ALIGN" if self.eat(b'(') => {
                        let a = self.expr()?;
                        self.expect(b',')?;
                        let n = self.expr()?;
                        self.expect(b')')?;
                        Ok(LExpr::Align(Box::new(a), Box::new(n)))
                    }
                    _ => Ok(LExpr::Sym(w)),
                }
            }
            None => self.err("expected an expression"),
        }
    }
}

/// Parses a layout number: decimal, `0x` hex or `0b` binary, with an
/// optional `K` (1024) or `M` (1024*1024) suffix.
pub fn parse_number(text: &str) -> Option<i64> {
    // K and M are not hex digits, so a suffix is never ambiguous.
    let (body, mult) = match text.as_bytes().last()? {
        b'K' | b'k' => (&text[..text.len() - 1], 1024),
        b'M' | b'm' => (&text[..text.len() - 1], 1024 * 1024),
        _ => (text, 1),
    };
    let v = if let Some(h) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else if let Some(b) = body.strip_prefix("0b").or_else(|| body.strip_prefix("0B")) {
        i64::from_str_radix(b, 2).ok()?
    } else {
        body.parse::<i64>().ok()?
    };
    v.checked_mul(mult)
}

/// Evaluates a layout expression using only numbers and region queries
/// (used for `origin=`, `length=`, `at=`, `align=`).
fn eval_static(e: &LExpr, regions: &[Region]) -> Result<i64, String> {
    Ok(match e {
        LExpr::Num(v) => *v,
        LExpr::Query(q, name) if q == "ORIGIN" || q == "LENGTH" => {
            let r = regions
                .iter()
                .find(|r| &r.name == name)
                .ok_or(format!("unknown region '{name}'"))?;
            (if q == "ORIGIN" { r.origin } else { r.length }) as i64
        }
        LExpr::Neg(a) => eval_static(a, regions)?.wrapping_neg(),
        LExpr::Align(a, n) => align_up_i64(eval_static(a, regions)?, eval_static(n, regions)?)?,
        LExpr::Bin(op, a, b) => bin(*op, eval_static(a, regions)?, eval_static(b, regions)?)?,
        _ => return Err("only numbers, ORIGIN() and LENGTH() are allowed here".to_string()),
    })
}

/// `ALIGN(value, n)`.
pub fn align_up_i64(v: i64, n: i64) -> Result<i64, String> {
    if n <= 0 || n & (n - 1) != 0 {
        return Err(format!("ALIGN() requires a power of two, not {n}"));
    }
    Ok(v.wrapping_add(n - 1) & !(n - 1))
}

/// Applies a layout binary operator.
pub fn bin(op: char, a: i64, b: i64) -> Result<i64, String> {
    Ok(match op {
        '+' => a.wrapping_add(b),
        '-' => a.wrapping_sub(b),
        '*' => a.wrapping_mul(b),
        '/' if b == 0 => return Err("division by zero".to_string()),
        '/' => a.wrapping_div(b),
        '&' => a & b,
        _ => a | b,
    })
}

/// Parses layout text. Errors are reported to `diags`; `None` if any.
pub fn parse_layout(text: &str, file: FileId, diags: &mut Diags) -> Option<Layout> {
    let mut c = Cursor {
        s: text.as_bytes(),
        pos: 0,
        line: 1,
        line_start: 0,
        file,
        depth: 0,
    };
    let mut layout = Layout::default();
    let mut ok = true;
    loop {
        c.skip(true);
        if c.peek().is_none() {
            break;
        }
        if let Err((loc, msg)) = statement(&mut c, &mut layout) {
            diags.error(loc, msg);
            ok = false;
            // Resynchronize at the next line.
            while c.peek().is_some_and(|ch| ch != b'\n') {
                c.pos += 1;
            }
        }
    }
    ok.then_some(layout)
}

/// Parses the patterns of an output rule after its `{`, up to and including
/// the closing `}`: glob words, and `KEEP(glob...)` groups.
fn pattern_list(c: &mut Cursor) -> PResult<Vec<Pattern>> {
    let mut patterns = Vec::new();
    // Location of an open `KEEP(` and the pattern count when it opened.
    let mut keep: Option<(Loc, usize)> = None;
    loop {
        c.skip(true);
        match c.peek() {
            None => return c.err("missing '}'"),
            Some(b'}') => {
                if let Some((loc, _)) = keep {
                    return Err((loc, "KEEP( is missing its ')'".to_string()));
                }
                c.pos += 1;
                return Ok(patterns);
            }
            Some(b')') if keep.is_some() => {
                let (loc, count) = keep.take().unwrap_or_default();
                if patterns.len() == count {
                    return Err((loc, "KEEP() needs at least one pattern".to_string()));
                }
                c.pos += 1;
            }
            Some(b'(' | b')') => return c.err("unexpected parenthesis in the pattern list"),
            Some(_) => {
                let loc = c.loc();
                let start = c.pos;
                while c
                    .peek()
                    .is_some_and(|ch| !ch.is_ascii_whitespace() && !b"{}()#".contains(&ch))
                {
                    c.pos += 1;
                }
                let word = String::from_utf8_lossy(&c.s[start..c.pos]).into_owned();
                if word == "KEEP" && keep.is_none() {
                    c.expect(b'(')?;
                    keep = Some((loc, patterns.len()));
                } else {
                    patterns.push(Pattern {
                        glob: word,
                        keep: keep.is_some(),
                    });
                }
            }
        }
    }
}

fn statement(c: &mut Cursor, layout: &mut Layout) -> PResult<()> {
    let loc = c.loc();
    let kw = c.expect_word("'memory', 'output', 'define' or 'entry'")?;
    match kw.as_str() {
        "memory" => {
            let name = c.expect_word("a region name")?;
            if layout.region_index(&name).is_some() {
                return Err((loc, format!("region '{name}' is defined twice")));
            }
            let (mut origin, mut length) = (None, None);
            while !c.at_line_end() {
                let kloc = c.loc();
                let key = c.expect_word("'origin=' or 'length='")?;
                c.expect(b'=')?;
                let e = c.expr()?;
                let v = eval_static(&e, &layout.regions).map_err(|m| (kloc, m))?;
                if !(0..=1i64 << 32).contains(&v) {
                    return Err((kloc, format!("{key} is outside the 32-bit address space")));
                }
                match key.as_str() {
                    "origin" => origin = Some(v as u64),
                    "length" => length = Some(v as u64),
                    _ => return Err((kloc, format!("unknown memory attribute '{key}'"))),
                }
            }
            let (Some(origin), Some(length)) = (origin, length) else {
                return Err((loc, "memory requires origin= and length=".to_string()));
            };
            if origin + length > 1 << 32 {
                return Err((
                    loc,
                    format!("region '{name}' extends beyond the 32-bit address space"),
                ));
            }
            layout.regions.push(Region {
                name,
                origin,
                length,
                loc,
            });
        }
        "entry" => {
            let sym = c.expect_word("a symbol name")?;
            layout.entry = Some((sym, loc));
            if !c.at_line_end() {
                return c.err("unexpected text after entry symbol");
            }
        }
        "define" => {
            let name = c.expect_word("a symbol name")?;
            c.expect(b'=')?;
            let expr = c.expr()?;
            if !c.at_line_end() {
                return c.err("unexpected text after expression");
            }
            layout.defines.push(Define { name, expr, loc });
        }
        "output" => {
            let name = c.expect_word("an output section name")?;
            if layout.outputs.iter().any(|o| o.name == name) {
                return Err((loc, format!("output section '{name}' is defined twice")));
            }
            if c.word().as_deref() != Some("in") {
                return c.err("expected 'in REGION'");
            }
            let rloc = c.loc();
            let rname = c.expect_word("a region name")?;
            let region = layout
                .region_index(&rname)
                .ok_or((rloc, format!("unknown region '{rname}'")))?;
            let (mut at, mut align, mut nobits) = (None, 1u64, false);
            loop {
                c.skip(true);
                if c.peek() == Some(b'{') {
                    c.pos += 1;
                    break;
                }
                let kloc = c.loc();
                let key = c.expect_word("'at=', 'align=', 'nobits' or '{'")?;
                match key.as_str() {
                    "nobits" => nobits = true,
                    "at" | "align" => {
                        c.expect(b'=')?;
                        let e = c.expr()?;
                        let v = eval_static(&e, &layout.regions).map_err(|m| (kloc, m))?;
                        if key == "at" {
                            if !(0..1i64 << 32).contains(&v) {
                                return Err((
                                    kloc,
                                    "at= is outside the 32-bit address space".to_string(),
                                ));
                            }
                            at = Some(v as u64);
                        } else {
                            if v <= 0 || v & (v - 1) != 0 || v > 1 << 30 {
                                return Err((kloc, "align= must be a power of two".to_string()));
                            }
                            align = v as u64;
                        }
                    }
                    _ => return Err((kloc, format!("unknown output attribute '{key}'"))),
                }
            }
            let patterns = pattern_list(c)?;
            layout.outputs.push(OutputRule {
                name,
                region,
                at,
                align,
                nobits,
                patterns,
                loc,
            });
            if !c.at_line_end() {
                return c.err("unexpected text after '}'");
            }
        }
        other => return Err((loc, format!("unknown layout statement '{other}'"))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "# comments start with '#'
memory RAM  origin=0x10000000 length=16K
entry _start

output .text   in RAM { .text.start .text .text.* }
output .rodata in RAM align=4 { .rodata .rodata.* .srodata .srodata.* }
output .data   in RAM align=4 { .data .data.* .sdata .sdata.* }
output .bss    in RAM align=4 nobits { .bss .bss.* .sbss .sbss.* COMMON }

define __bss_start = START(.bss)
define __bss_end   = END(.bss)
define __stack_top = ORIGIN(RAM) + LENGTH(RAM)
";

    #[test]
    fn parses_spec_example() {
        let mut d = Diags::new();
        let l = parse_layout(EXAMPLE, FileId(0), &mut d).expect("parses");
        assert!(!d.has_errors());
        assert_eq!(l.regions.len(), 1);
        assert_eq!(
            (l.regions[0].origin, l.regions[0].length),
            (0x1000_0000, 16 * 1024)
        );
        assert_eq!(l.entry.as_ref().map(|e| e.0.as_str()), Some("_start"));
        assert_eq!(l.outputs.len(), 4);
        assert_eq!(
            globs(&l.outputs[0].patterns),
            vec![".text.start", ".text", ".text.*"]
        );
        assert_eq!(l.outputs[1].align, 4);
        assert!(l.outputs[3].nobits);
        assert_eq!(
            l.outputs[3].patterns.last().map(|p| p.glob.as_str()),
            Some("COMMON")
        );
        assert_eq!(l.defines.len(), 3);
        assert_eq!(
            l.defines[2].expr,
            LExpr::Bin(
                '+',
                Box::new(LExpr::Query("ORIGIN".into(), "RAM".into())),
                Box::new(LExpr::Query("LENGTH".into(), "RAM".into()))
            )
        );
    }

    #[test]
    fn multiline_braces_and_at() {
        let text = "memory ROM origin=0 length=4K\nmemory RAM origin=ORIGIN(ROM)+0x10000000 length=1M\noutput .text in ROM at=0x100 {\n  .text # code\n  .text.*\n}\n";
        let mut d = Diags::new();
        let l = parse_layout(text, FileId(0), &mut d).expect("parses");
        assert_eq!(l.regions[1].origin, 0x1000_0000);
        assert_eq!(l.regions[1].length, 1 << 20);
        assert_eq!(l.outputs[0].at, Some(0x100));
        assert_eq!(globs(&l.outputs[0].patterns), vec![".text", ".text.*"]);
    }

    fn globs(p: &[Pattern]) -> Vec<&str> {
        p.iter().map(|p| p.glob.as_str()).collect()
    }

    #[test]
    fn keep_patterns() {
        let text = "memory S origin=0 length=8K\noutput .text in S { KEEP(.text.header) .text KEEP( .init .fini\n) .text.* }\n";
        let mut d = Diags::new();
        let l = parse_layout(text, FileId(0), &mut d).expect("parses");
        let pats: Vec<(&str, bool)> = l.outputs[0]
            .patterns
            .iter()
            .map(|p| (p.glob.as_str(), p.keep))
            .collect();
        assert_eq!(
            pats,
            vec![
                (".text.header", true),
                (".text", false),
                (".init", true),
                (".fini", true),
                (".text.*", false)
            ]
        );
        for (bad, msg) in [
            ("output .t in S { KEEP(.x }", "missing its ')'"),
            ("output .t in S { KEEP() }", "at least one pattern"),
            ("output .t in S { KEEP .x }", "expected '('"),
            ("output .t in S { .a) }", "unexpected parenthesis"),
        ] {
            let mut d = Diags::new();
            assert!(parse_layout(
                &format!("memory S origin=0 length=8K\n{bad}\n"),
                FileId(0),
                &mut d
            )
            .is_none());
            assert!(
                d.list.iter().any(|x| x.message.contains(msg)),
                "{bad}: {:?}",
                d.list
            );
        }
    }

    #[test]
    fn errors_have_locations() {
        let mut d = Diags::new();
        assert!(parse_layout(
            "memory RAM origin=0x10\noutput .t in NOPE { .t }\nbogus\n",
            FileId(0),
            &mut d
        )
        .is_none());
        let msgs: Vec<_> = d
            .list
            .iter()
            .map(|x| (x.loc.map(|l| l.line), x.message.clone()))
            .collect();
        assert_eq!(msgs.len(), 3, "{msgs:?}");
        assert_eq!(msgs[0].0, Some(1));
        assert!(msgs[1].1.contains("unknown region"));
        assert_eq!(msgs[2].0, Some(3));
    }

    #[test]
    fn numbers_and_globs() {
        assert_eq!(parse_number("16K"), Some(16384));
        assert_eq!(parse_number("2M"), Some(2 << 20));
        assert_eq!(parse_number("0x1000"), Some(4096));
        assert_eq!(parse_number("0b101"), Some(5));
        assert_eq!(parse_number("12Q"), None);
        assert!(glob_match(".text.*", ".text.foo"));
        assert!(!glob_match(".text.*", ".text"));
        assert!(glob_match("*", ".anything"));
        assert!(glob_match(".t?xt", ".text"));
        assert!(glob_match("*.str1.*", ".rodata.str1.1"));
        assert!(!glob_match(".data", ".data1"));
    }
}
