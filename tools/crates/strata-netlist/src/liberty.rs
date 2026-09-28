//! A parser for the Liberty (`.lib`) format, producing a generic group tree,
//! plus typed views of the parts Strata uses: cells, pins, pin functions,
//! capacitances, flip-flop definitions and timing tables.

use std::fmt;

/// An attribute value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `name : value ;`
    Simple(String),
    /// `name (arg, arg, ...) ;`
    Complex(Vec<String>),
}

/// A Liberty group: `kind (args) { attributes and subgroups }`.
#[derive(Debug, Clone, Default)]
pub struct Group {
    pub kind: String,
    pub args: Vec<String>,
    pub attrs: Vec<(String, Value)>,
    pub groups: Vec<Group>,
}

impl Group {
    /// First simple attribute with this name.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find_map(|(n, v)| match v {
            Value::Simple(s) if n == name => Some(s.as_str()),
            _ => None,
        })
    }

    /// First complex attribute with this name.
    pub fn complex(&self, name: &str) -> Option<&[String]> {
        self.attrs.iter().find_map(|(n, v)| match v {
            Value::Complex(a) if n == name => Some(a.as_slice()),
            _ => None,
        })
    }

    pub fn attr_f64(&self, name: &str) -> Option<f64> {
        self.attr(name).and_then(|s| s.parse().ok())
    }

    /// Subgroups of a kind.
    pub fn groups_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Group> + 'a {
        self.groups.iter().filter(move |g| g.kind == kind)
    }

    pub fn group(&self, kind: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.kind == kind)
    }

    /// The first argument, which names most groups (cells, pins).
    pub fn name(&self) -> &str {
        self.args.first().map(String::as_str).unwrap_or("")
    }
}

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "liberty:{}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Punct(u8),
}

struct Lexer<'a> {
    s: &'a [u8],
    pos: usize,
    line: usize,
}

impl<'a> Lexer<'a> {
    fn skip_ws(&mut self) {
        loop {
            let Some(&c) = self.s.get(self.pos) else { return };
            match c {
                b'\n' => {
                    self.line += 1;
                    self.pos += 1;
                }
                b' ' | b'\t' | b'\r' => self.pos += 1,
                // Line continuation.
                b'\\' if matches!(self.s.get(self.pos + 1), Some(b'\n') | Some(b'\r')) => {
                    self.pos += 1;
                }
                b'/' if self.s.get(self.pos + 1) == Some(&b'*') => {
                    self.pos += 2;
                    while self.pos < self.s.len()
                        && !(self.s[self.pos] == b'*' && self.s.get(self.pos + 1) == Some(&b'/'))
                    {
                        if self.s[self.pos] == b'\n' {
                            self.line += 1;
                        }
                        self.pos += 1;
                    }
                    self.pos += 2;
                }
                b'/' if self.s.get(self.pos + 1) == Some(&b'/') => {
                    while self.pos < self.s.len() && self.s[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    fn next(&mut self) -> Result<Option<Tok>, ParseError> {
        self.skip_ws();
        let Some(&c) = self.s.get(self.pos) else { return Ok(None) };
        match c {
            b'{' | b'}' | b'(' | b')' | b':' | b';' | b',' => {
                self.pos += 1;
                Ok(Some(Tok::Punct(c)))
            }
            b'"' => {
                self.pos += 1;
                let mut out = Vec::new();
                while let Some(&c) = self.s.get(self.pos) {
                    match c {
                        b'"' => break,
                        b'\\' if matches!(self.s.get(self.pos + 1), Some(b'\n') | Some(b'\r')) => {
                            self.pos += 1;
                        }
                        b'\n' => {
                            self.line += 1;
                            self.pos += 1;
                        }
                        _ => {
                            out.push(c);
                            self.pos += 1;
                        }
                    }
                }
                if self.pos >= self.s.len() {
                    return Err(self.err("unterminated string"));
                }
                self.pos += 1;
                Ok(Some(Tok::Str(String::from_utf8_lossy(&out).into_owned())))
            }
            _ => {
                let start = self.pos;
                while let Some(&c) = self.s.get(self.pos) {
                    if c.is_ascii_whitespace() || b"{}():;,\"".contains(&c) {
                        break;
                    }
                    self.pos += 1;
                }
                Ok(Some(Tok::Word(String::from_utf8_lossy(&self.s[start..self.pos]).into_owned())))
            }
        }
    }

    fn err(&self, msg: &str) -> ParseError {
        ParseError { line: self.line, msg: msg.to_string() }
    }
}

struct Parser<'a> {
    lx: Lexer<'a>,
    peeked: Option<Tok>,
}

impl<'a> Parser<'a> {
    fn peek(&mut self) -> Result<Option<&Tok>, ParseError> {
        if self.peeked.is_none() {
            self.peeked = self.lx.next()?;
        }
        Ok(self.peeked.as_ref())
    }

    fn take(&mut self) -> Result<Option<Tok>, ParseError> {
        if let Some(t) = self.peeked.take() {
            return Ok(Some(t));
        }
        self.lx.next()
    }

    fn word(&mut self) -> Result<String, ParseError> {
        match self.take()? {
            Some(Tok::Word(w)) | Some(Tok::Str(w)) => Ok(w),
            other => Err(self.lx.err(&format!("expected a name, found {other:?}"))),
        }
    }

    /// Arguments after '(' up to ')'.
    fn args(&mut self) -> Result<Vec<String>, ParseError> {
        let mut out = Vec::new();
        loop {
            match self.take()? {
                Some(Tok::Punct(b')')) => return Ok(out),
                Some(Tok::Punct(b',')) => {}
                Some(Tok::Word(w)) | Some(Tok::Str(w)) => out.push(w),
                other => return Err(self.lx.err(&format!("bad argument list near {other:?}"))),
            }
        }
    }

    fn skip_semi(&mut self) -> Result<(), ParseError> {
        if let Some(Tok::Punct(b';')) = self.peek()? {
            self.take()?;
        }
        Ok(())
    }

    /// Parses statements until '}' (consumed) or end of input.
    fn body(&mut self, g: &mut Group) -> Result<(), ParseError> {
        loop {
            match self.peek()? {
                None => return Ok(()),
                Some(Tok::Punct(b'}')) => {
                    self.take()?;
                    return Ok(());
                }
                Some(Tok::Punct(b';')) => {
                    self.take()?;
                    continue;
                }
                _ => {}
            }
            let name = self.word()?;
            match self.take()? {
                Some(Tok::Punct(b':')) => {
                    // Simple attribute: the value runs until ';' or end of line group.
                    let mut val = String::new();
                    loop {
                        match self.peek()? {
                            Some(Tok::Punct(b';')) | Some(Tok::Punct(b'}')) | None => break,
                            _ => {}
                        }
                        match self.take()? {
                            Some(Tok::Word(w)) | Some(Tok::Str(w)) => {
                                if !val.is_empty() {
                                    val.push(' ');
                                }
                                val.push_str(&w);
                            }
                            Some(Tok::Punct(c)) => val.push(c as char),
                            None => break,
                        }
                        // Values are a single token in practice; stop at a new statement.
                        if let Some(Tok::Word(_)) = self.peek()? {
                            break;
                        }
                    }
                    self.skip_semi()?;
                    g.attrs.push((name, Value::Simple(val)));
                }
                Some(Tok::Punct(b'(')) => {
                    let args = self.args()?;
                    if let Some(Tok::Punct(b'{')) = self.peek()? {
                        self.take()?;
                        let mut sub = Group { kind: name, args, ..Default::default() };
                        self.body(&mut sub)?;
                        g.groups.push(sub);
                    } else {
                        self.skip_semi()?;
                        g.attrs.push((name, Value::Complex(args)));
                    }
                }
                other => {
                    return Err(self.lx.err(&format!("unexpected {other:?} after '{name}'")));
                }
            }
        }
    }
}

/// Parses Liberty source text. Returns the top-level `library` group.
pub fn parse(src: &str) -> Result<Group, ParseError> {
    let mut p = Parser { lx: Lexer { s: src.as_bytes(), pos: 0, line: 1 }, peeked: None };
    let mut root = Group::default();
    p.body(&mut root)?;
    root.groups
        .into_iter()
        .find(|g| g.kind == "library")
        .ok_or_else(|| ParseError { line: 1, msg: "no library group".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_groups_and_attributes() {
        let src = r#"
            /* comment */
            library (lib) {
              time_unit : "1ns" ;
              capacitive_load_unit (1, pf);
              cell ("inv") {
                area : 3.75;
                pin ("A") { direction : input; capacitance : 0.0023; }
                pin ("Y") { direction : output; function : "(!A)";
                  timing () { related_pin : "A";
                    cell_rise ("tpl") { index_1 ("0.01, \
                      0.02"); values ("1, 2"); }
                  }
                }
              }
            }"#;
        let lib = parse(src).unwrap();
        assert_eq!(lib.name(), "lib");
        assert_eq!(lib.complex("capacitive_load_unit").unwrap(), &["1", "pf"]);
        let cell = lib.groups_of("cell").next().unwrap();
        assert_eq!(cell.name(), "inv");
        assert_eq!(cell.attr_f64("area"), Some(3.75));
        let y = cell.groups_of("pin").find(|p| p.name() == "Y").unwrap();
        assert_eq!(y.attr("function"), Some("(!A)"));
        let t = y.group("timing").unwrap().group("cell_rise").unwrap();
        let idx: Vec<&str> = t.complex("index_1").unwrap()[0].split(',').map(str::trim).collect();
        assert_eq!(idx, ["0.01", "0.02"]);
    }
}
