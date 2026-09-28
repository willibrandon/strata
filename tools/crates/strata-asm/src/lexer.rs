//! Tokenizer for one assembler statement.

use std::fmt;

/// Punctuation and operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Punct {
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Shl,
    Shr,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Bang,
    EqEq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    AndAnd,
    OrOr,
    Assign,
    At,
}

impl Punct {
    pub fn as_str(self) -> &'static str {
        use Punct::*;
        match self {
            LParen => "(",
            RParen => ")",
            LBrace => "{",
            RBrace => "}",
            Comma => ",",
            Colon => ":",
            Plus => "+",
            Minus => "-",
            Star => "*",
            Slash => "/",
            Percent => "%",
            Shl => "<<",
            Shr => ">>",
            Amp => "&",
            Pipe => "|",
            Caret => "^",
            Tilde => "~",
            Bang => "!",
            EqEq => "==",
            Ne => "!=",
            Lt => "<",
            Gt => ">",
            Le => "<=",
            Ge => ">=",
            AndAnd => "&&",
            OrOr => "||",
            Assign => "=",
            At => "@",
        }
    }
}

/// A token kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    /// Identifier, directive name, register, or `.` (`[A-Za-z_.$][A-Za-z0-9_.$]*`).
    Ident(String),
    /// Integer or character literal (64-bit, wrapping from unsigned).
    Int(i64),
    /// Integer literal wider than 64 bits (only meaningful to directives such
    /// as `.file ... md5 0x...`; expressions reject it).
    BigInt(String),
    /// String literal with escapes processed.
    Str(Vec<u8>),
    /// Numeric local label reference `Nb` / `Nf`.
    NumLabel {
        n: u32,
        forward: bool,
    },
    Punct(Punct),
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Ident(s) => write!(f, "'{s}'"),
            Tok::Int(v) => write!(f, "'{v}'"),
            Tok::BigInt(s) => write!(f, "'{s}'"),
            Tok::Str(_) => write!(f, "string"),
            Tok::NumLabel { n, forward } => write!(f, "'{n}{}'", if *forward { 'f' } else { 'b' }),
            Tok::Punct(p) => write!(f, "'{}'", p.as_str()),
        }
    }
}

/// A token with its byte column (0-based) within the statement text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub tok: Tok,
    pub col: u32,
    /// Byte length of the token's source text.
    pub len: u32,
}

impl Token {
    pub fn is_punct(&self, p: Punct) -> bool {
        self.tok == Tok::Punct(p)
    }

    pub fn ident(&self) -> Option<&str> {
        match &self.tok {
            Tok::Ident(s) => Some(s),
            _ => None,
        }
    }
}

/// A lexical error: byte column and message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    pub col: u32,
    pub msg: String,
}

pub fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'.' || c == b'$'
}

pub fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'$'
}

/// Tokenizes a statement (comments already removed).
pub fn tokenize(text: &str) -> Result<Vec<Token>, LexError> {
    let b = text.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let err = |msg: &str| LexError {
            col: start as u32,
            msg: msg.to_string(),
        };
        let tok = if is_ident_start(c) {
            while i < b.len() && is_ident_char(b[i]) {
                i += 1;
            }
            Tok::Ident(text[start..i].to_string())
        } else if c.is_ascii_digit() {
            let (tok, end) = lex_number(b, i).map_err(|m| err(&m))?;
            i = end;
            tok
        } else if c == b'"' {
            let (bytes, end) = lex_string(b, i + 1).map_err(|(off, m)| LexError {
                col: off as u32,
                msg: m,
            })?;
            i = end;
            Tok::Str(bytes)
        } else if c == b'\'' {
            i += 1;
            let (v, end) = lex_char_body(b, i).map_err(|m| err(&m))?;
            i = end;
            if b.get(i) == Some(&b'\'') {
                i += 1;
            }
            Tok::Int(v)
        } else {
            let two = if i + 1 < b.len() {
                &b[i..i + 2]
            } else {
                &b[i..i + 1]
            };
            let (p, n) = match two {
                b"<<" => (Punct::Shl, 2),
                b">>" => (Punct::Shr, 2),
                b"==" => (Punct::EqEq, 2),
                b"!=" => (Punct::Ne, 2),
                b"<=" => (Punct::Le, 2),
                b">=" => (Punct::Ge, 2),
                b"&&" => (Punct::AndAnd, 2),
                b"||" => (Punct::OrOr, 2),
                b"<>" => (Punct::Ne, 2),
                _ => {
                    let p = match c {
                        b'(' => Punct::LParen,
                        b')' => Punct::RParen,
                        b'{' => Punct::LBrace,
                        b'}' => Punct::RBrace,
                        b',' => Punct::Comma,
                        b':' => Punct::Colon,
                        b'+' => Punct::Plus,
                        b'-' => Punct::Minus,
                        b'*' => Punct::Star,
                        b'/' => Punct::Slash,
                        b'%' => Punct::Percent,
                        b'&' => Punct::Amp,
                        b'|' => Punct::Pipe,
                        b'^' => Punct::Caret,
                        b'~' => Punct::Tilde,
                        b'!' => Punct::Bang,
                        b'<' => Punct::Lt,
                        b'>' => Punct::Gt,
                        b'=' => Punct::Assign,
                        b'@' => Punct::At,
                        _ => {
                            let ch = text[i..].chars().next().unwrap_or('?');
                            return Err(err(&format!("unexpected character '{ch}'")));
                        }
                    };
                    (p, 1)
                }
            };
            i += n;
            Tok::Punct(p)
        };
        toks.push(Token {
            tok,
            col: start as u32,
            len: (i - start) as u32,
        });
    }
    Ok(toks)
}

fn lex_number(b: &[u8], start: usize) -> Result<(Tok, usize), String> {
    let mut i = start;
    let digits_end = |mut j: usize, pred: fn(u8) -> bool| {
        while j < b.len() && pred(b[j]) {
            j += 1;
        }
        j
    };
    let (radix, body_start) = if b[i] == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
        (16, i + 2)
    } else if b[i] == b'0'
        && matches!(b.get(i + 1), Some(b'b' | b'B'))
        && matches!(b.get(i + 2), Some(b'0' | b'1'))
    {
        (2, i + 2)
    } else {
        // Decimal digits first; they may turn out to be a local label reference.
        let end = digits_end(i, |c| c.is_ascii_digit());
        if let Some(&suffix @ (b'b' | b'f')) = b.get(end) {
            if !b.get(end + 1).is_some_and(|&c| is_ident_char(c)) {
                let n: u32 = std::str::from_utf8(&b[i..end])
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .ok_or("local label number too large")?;
                return Ok((
                    Tok::NumLabel {
                        n,
                        forward: suffix == b'f',
                    },
                    end + 1,
                ));
            }
        }
        if b[i] == b'0' && end > i + 1 {
            (8, i + 1)
        } else {
            (10, i)
        }
    };
    i = body_start;
    let end = digits_end(i, |c| c.is_ascii_hexdigit());
    if end == i {
        return Err("invalid number: missing digits".to_string());
    }
    if b.get(end).is_some_and(|&c| is_ident_char(c)) {
        return Err("invalid number".to_string());
    }
    let mut value: Option<u64> = Some(0);
    for &c in &b[i..end] {
        let d = (c as char).to_digit(16).unwrap_or(99);
        if d >= radix {
            return Err(format!(
                "invalid digit '{}' in base-{radix} number",
                c as char
            ));
        }
        value = value
            .and_then(|v| v.checked_mul(u64::from(radix)))
            .and_then(|v| v.checked_add(u64::from(d)));
    }
    match value {
        Some(v) => Ok((Tok::Int(v as i64), end)),
        None => Ok((
            Tok::BigInt(String::from_utf8_lossy(&b[start..end]).into_owned()),
            end,
        )),
    }
}

/// Parses one (possibly escaped) character at `i`, returning its value and the
/// index after it.
fn lex_char_body(b: &[u8], i: usize) -> Result<(i64, usize), String> {
    match b.get(i) {
        None => Err("unterminated character literal".to_string()),
        Some(b'\\') => {
            let (v, end) = lex_escape(b, i + 1)?;
            Ok((i64::from(v), end))
        }
        Some(_) => {
            // A multi-byte UTF-8 character yields its first byte, like GNU as.
            Ok((i64::from(b[i]), i + 1))
        }
    }
}

/// Parses the escape sequence after a backslash at `i`.
fn lex_escape(b: &[u8], i: usize) -> Result<(u8, usize), String> {
    let c = *b.get(i).ok_or("unterminated escape sequence")?;
    Ok(match c {
        b'n' => (b'\n', i + 1),
        b't' => (b'\t', i + 1),
        b'r' => (b'\r', i + 1),
        b'a' => (7, i + 1),
        b'b' => (8, i + 1),
        b'f' => (12, i + 1),
        b'v' => (11, i + 1),
        b'e' => (27, i + 1),
        b'0'..=b'7' => {
            let mut v: u32 = 0;
            let mut j = i;
            while j < b.len() && j < i + 3 && (b'0'..=b'7').contains(&b[j]) {
                v = v * 8 + u32::from(b[j] - b'0');
                j += 1;
            }
            ((v & 0xFF) as u8, j)
        }
        b'x' | b'X' => {
            let mut v: u32 = 0;
            let mut j = i + 1;
            while j < b.len() && j < i + 3 && b[j].is_ascii_hexdigit() {
                v = v * 16 + (b[j] as char).to_digit(16).unwrap_or(0);
                j += 1;
            }
            if j == i + 1 {
                return Err("\\x used with no following hex digits".to_string());
            }
            (v as u8, j)
        }
        // \\, \", \' and any other character stand for themselves.
        other => (other, i + 1),
    })
}

/// Parses a string body starting after the opening quote.
fn lex_string(b: &[u8], mut i: usize) -> Result<(Vec<u8>, usize), (usize, String)> {
    let open = i - 1;
    let mut out = Vec::new();
    loop {
        match b.get(i) {
            None => return Err((open, "unterminated string".to_string())),
            Some(b'"') => return Ok((out, i + 1)),
            Some(b'\\') => {
                let (v, end) = lex_escape(b, i + 1).map_err(|m| (i, m))?;
                out.push(v);
                i = end;
            }
            Some(&c) => {
                out.push(c);
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    fn id(s: &str) -> Tok {
        Tok::Ident(s.to_string())
    }

    #[test]
    fn numbers() {
        assert_eq!(
            toks("0x1F 0b101 017 42 0"),
            vec![
                Tok::Int(31),
                Tok::Int(5),
                Tok::Int(15),
                Tok::Int(42),
                Tok::Int(0)
            ]
        );
        assert_eq!(toks("0xFFFFFFFFFFFFFFFF"), vec![Tok::Int(-1)]);
        assert_eq!(
            toks("0x10000000000000000"),
            vec![Tok::BigInt("0x10000000000000000".into())]
        );
        assert!(tokenize("09").is_err());
        assert!(tokenize("12ab").is_err());
        assert!(tokenize("0x").is_err());
    }

    #[test]
    fn numeric_labels() {
        assert_eq!(
            toks("1b 23f 0b 0f"),
            vec![
                Tok::NumLabel {
                    n: 1,
                    forward: false
                },
                Tok::NumLabel {
                    n: 23,
                    forward: true
                },
                Tok::NumLabel {
                    n: 0,
                    forward: false
                },
                Tok::NumLabel {
                    n: 0,
                    forward: true
                },
            ]
        );
    }

    #[test]
    fn chars_and_strings() {
        assert_eq!(
            toks("'a' '\\n' '\\0' '\\\\' '\\''"),
            vec![
                Tok::Int(97),
                Tok::Int(10),
                Tok::Int(0),
                Tok::Int(92),
                Tok::Int(39)
            ]
        );
        assert_eq!(
            toks("\"a\\tb\\000\\x41\\\"\""),
            vec![Tok::Str(b"a\tb\0A\"".to_vec())]
        );
        assert_eq!(toks("\"\\342\\200\""), vec![Tok::Str(vec![0o342, 0o200])]);
        assert!(tokenize("\"abc").is_err());
    }

    #[test]
    fn operators_and_idents() {
        assert_eq!(
            toks("lw a0, %lo(sym+4)(a1)"),
            vec![
                id("lw"),
                id("a0"),
                Tok::Punct(Punct::Comma),
                Tok::Punct(Punct::Percent),
                id("lo"),
                Tok::Punct(Punct::LParen),
                id("sym"),
                Tok::Punct(Punct::Plus),
                Tok::Int(4),
                Tok::Punct(Punct::RParen),
                Tok::Punct(Punct::LParen),
                id("a1"),
                Tok::Punct(Punct::RParen),
            ]
        );
        assert_eq!(
            toks("a<<b>=c!=d&&.L$x_1"),
            vec![
                id("a"),
                Tok::Punct(Punct::Shl),
                id("b"),
                Tok::Punct(Punct::Ge),
                id("c"),
                Tok::Punct(Punct::Ne),
                id("d"),
                Tok::Punct(Punct::AndAnd),
                id(".L$x_1"),
            ]
        );
        let e = tokenize("a ? b").unwrap_err();
        assert_eq!(e.col, 2);
    }
}
