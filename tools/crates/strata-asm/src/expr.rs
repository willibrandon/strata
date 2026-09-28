//! Assembler expressions: AST, parser (C operator precedence), and a
//! symbolic evaluator that tracks whether a value is absolute or relative to
//! a section.

use crate::lexer::{Punct, Tok, Token};

/// Index of a symbol within one object's symbol table.
pub type SymId = u32;

/// A section of a particular object (input file or synthetic object).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SecRef {
    pub obj: u32,
    pub sec: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    LNot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    And,
    Or,
    Xor,
    Add,
    Sub,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    LAnd,
    LOr,
}

/// An expression tree. Symbols are bound to their object's symbol table at
/// parse time; `.` becomes a fixed [`Expr::Loc`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Num(i64),
    Sym(SymId),
    Loc(SecRef, i64),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

impl Expr {
    /// The constant value, if the tree is a literal number.
    pub fn as_num(&self) -> Option<i64> {
        match self {
            Expr::Num(v) => Some(*v),
            _ => None,
        }
    }

    /// Number of nodes, counting at most up to `limit` (then returns `limit`).
    pub fn size_up_to(&self, limit: u32) -> u32 {
        fn walk(e: &Expr, left: &mut u32) {
            if *left == 0 {
                return;
            }
            *left -= 1;
            match e {
                Expr::Unary(_, a) => walk(a, left),
                Expr::Binary(_, a, b) => {
                    walk(a, left);
                    walk(b, left);
                }
                _ => {}
            }
        }
        let mut left = limit;
        walk(self, &mut left);
        limit - left
    }

    /// True if the expression mentions no symbol and no location.
    pub fn is_constant(&self) -> bool {
        match self {
            Expr::Num(_) => true,
            Expr::Sym(_) | Expr::Loc(..) => false,
            Expr::Unary(_, e) => e.is_constant(),
            Expr::Binary(_, a, b) => a.is_constant() && b.is_constant(),
        }
    }
}

/// The value of an expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    /// A number that does not depend on where anything is placed.
    Abs(i64),
    /// `address of section + offset`.
    Rel(SecRef, i64),
}

/// Supplies symbol values and (after layout) section addresses to [`eval`].
pub trait Env {
    /// Value of symbol `id`, `Ok(None)` if it is not known (yet).
    /// `depth` must be passed back to [`eval`] when evaluating definitions.
    fn symbol(&self, id: SymId, depth: u32) -> Result<Option<Value>, String>;
    /// Final address of a section, once placement is known.
    fn section_addr(&self, sec: SecRef) -> Option<i64>;
}

/// Maximum nesting of symbol definitions (guards against cycles).
pub const MAX_DEPTH: u32 = 64;

/// Evaluates `e`. `Ok(None)` means some symbol is not known yet.
pub fn eval(e: &Expr, env: &dyn Env, depth: u32) -> Result<Option<Value>, String> {
    if depth > MAX_DEPTH {
        return Err("symbol definitions are circular or nested too deeply".to_string());
    }
    Ok(Some(match e {
        Expr::Num(v) => Value::Abs(*v),
        Expr::Loc(sec, off) => Value::Rel(*sec, *off),
        Expr::Sym(id) => return env.symbol(*id, depth + 1),
        Expr::Unary(op, a) => {
            let Some(v) = eval(a, env, depth)? else {
                return Ok(None);
            };
            let Value::Abs(x) = v else {
                return Err("unary operator applied to a relocatable value".to_string());
            };
            Value::Abs(match op {
                UnOp::Neg => x.wrapping_neg(),
                UnOp::Not => !x,
                UnOp::LNot => i64::from(x == 0),
            })
        }
        Expr::Binary(op, a, b) => {
            let va = eval(a, env, depth)?;
            let vb = eval(b, env, depth)?;
            let (Some(va), Some(vb)) = (va, vb) else {
                return Ok(None);
            };
            return binary(*op, va, vb, env);
        }
    }))
}

fn binary(op: BinOp, a: Value, b: Value, env: &dyn Env) -> Result<Option<Value>, String> {
    use Value::*;
    match (op, a, b) {
        (BinOp::Add, Rel(s, x), Abs(y)) | (BinOp::Add, Abs(y), Rel(s, x)) => {
            return Ok(Some(Rel(s, x.wrapping_add(y))))
        }
        (BinOp::Add, Rel(..), Rel(..)) => {
            return Err("cannot add two relocatable values".to_string())
        }
        (BinOp::Sub, Rel(s, x), Abs(y)) => return Ok(Some(Rel(s, x.wrapping_sub(y)))),
        (BinOp::Sub, Rel(s1, x), Rel(s2, y)) => {
            if s1 == s2 {
                return Ok(Some(Abs(x.wrapping_sub(y))));
            }
            // Symbols in different sections: known once both are placed.
            return Ok(match (env.section_addr(s1), env.section_addr(s2)) {
                (Some(p), Some(q)) => Some(Abs(p.wrapping_add(x).wrapping_sub(q.wrapping_add(y)))),
                _ => None,
            });
        }
        (BinOp::Sub, Abs(_), Rel(..)) => {
            return Err("cannot subtract a relocatable value from an absolute value".to_string())
        }
        _ => {}
    }
    let (Abs(x), Abs(y)) = (a, b) else {
        return Err(format!(
            "operator '{}' requires absolute operands",
            binop_str(op)
        ));
    };
    let t = |c: bool| if c { -1 } else { 0 };
    let v = match op {
        BinOp::Add => x.wrapping_add(y),
        BinOp::Sub => x.wrapping_sub(y),
        BinOp::Mul => x.wrapping_mul(y),
        BinOp::Div | BinOp::Rem if y == 0 => return Err("division by zero".to_string()),
        BinOp::Div => x.wrapping_div(y),
        BinOp::Rem => x.wrapping_rem(y),
        BinOp::Shl => {
            if (0..64).contains(&y) {
                x.wrapping_shl(y as u32)
            } else {
                0
            }
        }
        BinOp::Shr => {
            if (0..64).contains(&y) {
                ((x as u64) >> y) as i64
            } else {
                0
            }
        }
        BinOp::And => x & y,
        BinOp::Or => x | y,
        BinOp::Xor => x ^ y,
        BinOp::Eq => t(x == y),
        BinOp::Ne => t(x != y),
        BinOp::Lt => t(x < y),
        BinOp::Gt => t(x > y),
        BinOp::Le => t(x <= y),
        BinOp::Ge => t(x >= y),
        BinOp::LAnd => i64::from(x != 0 && y != 0),
        BinOp::LOr => i64::from(x != 0 || y != 0),
    };
    Ok(Some(Abs(v)))
}

fn binop_str(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Mul => "*",
        Div => "/",
        Rem => "%",
        Shl => "<<",
        Shr => ">>",
        And => "&",
        Or => "|",
        Xor => "^",
        Add => "+",
        Sub => "-",
        Eq => "==",
        Ne => "!=",
        Lt => "<",
        Gt => ">",
        Le => "<=",
        Ge => ">=",
        LAnd => "&&",
        LOr => "||",
    }
}

/// Binds names found while parsing.
pub trait Resolver {
    /// Resolves an identifier (a symbol name or `.`).
    fn ident(&mut self, name: &str) -> Result<Expr, String>;
    /// Resolves a numeric local label reference `Nb`/`Nf`.
    fn num_label(&mut self, n: u32, forward: bool) -> Result<Expr, String>;
}

/// A parse error at a byte column of the statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub col: u32,
    pub msg: String,
}

/// Largest number of operators and operands in one expression; keeps the
/// recursive parser and evaluator far from the stack limit.
pub const MAX_NODES: u32 = 4096;

/// Recursive-descent/precedence-climbing parser over a token slice.
pub struct Parser<'t> {
    /// Tokens of the expression (and possibly what follows it).
    pub toks: &'t [Token],
    /// Index of the next token.
    pub pos: usize,
    /// Column reported for errors at the end of input.
    pub end_col: u32,
    nodes: u32,
}

fn binop_of(p: Punct) -> Option<(BinOp, u8)> {
    // C precedence, higher binds tighter.
    Some(match p {
        Punct::OrOr => (BinOp::LOr, 1),
        Punct::AndAnd => (BinOp::LAnd, 2),
        Punct::Pipe => (BinOp::Or, 3),
        Punct::Caret => (BinOp::Xor, 4),
        Punct::Amp => (BinOp::And, 5),
        Punct::EqEq => (BinOp::Eq, 6),
        Punct::Ne => (BinOp::Ne, 6),
        Punct::Lt => (BinOp::Lt, 7),
        Punct::Gt => (BinOp::Gt, 7),
        Punct::Le => (BinOp::Le, 7),
        Punct::Ge => (BinOp::Ge, 7),
        Punct::Shl => (BinOp::Shl, 8),
        Punct::Shr => (BinOp::Shr, 8),
        Punct::Plus => (BinOp::Add, 9),
        Punct::Minus => (BinOp::Sub, 9),
        Punct::Star => (BinOp::Mul, 10),
        Punct::Slash => (BinOp::Div, 10),
        Punct::Percent => (BinOp::Rem, 10),
        _ => return None,
    })
}

impl<'t> Parser<'t> {
    /// A parser positioned at the first token.
    pub fn new(toks: &'t [Token], end_col: u32) -> Self {
        Parser {
            toks,
            pos: 0,
            end_col,
            nodes: 0,
        }
    }

    /// The next token, if any.
    pub fn peek(&self) -> Option<&'t Token> {
        self.toks.get(self.pos)
    }

    /// True when all tokens are consumed.
    pub fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    /// Column of the next token (or of the end of the statement).
    pub fn col(&self) -> u32 {
        self.peek().map_or(self.end_col, |t| t.col)
    }

    /// An error at the current position.
    pub fn error<T>(&self, msg: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            col: self.col(),
            msg: msg.into(),
        })
    }

    /// Consumes `p` if it is next.
    pub fn eat(&mut self, p: Punct) -> bool {
        if self.peek().is_some_and(|t| t.is_punct(p)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Consumes `p` or reports an error.
    pub fn expect(&mut self, p: Punct) -> Result<(), ParseError> {
        if self.eat(p) {
            Ok(())
        } else {
            match self.peek() {
                Some(t) => self.error(format!("expected '{}', found {}", p.as_str(), t.tok)),
                None => self.error(format!("expected '{}'", p.as_str())),
            }
        }
    }

    /// Parses a full expression.
    pub fn expr(&mut self, r: &mut dyn Resolver) -> Result<Expr, ParseError> {
        self.binary(1, r)
    }

    fn binary(&mut self, min_prec: u8, r: &mut dyn Resolver) -> Result<Expr, ParseError> {
        let mut lhs = self.unary(r)?;
        while let Some((op, prec)) = self.peek().and_then(|t| match t.tok {
            Tok::Punct(p) => binop_of(p),
            _ => None,
        }) {
            if prec < min_prec {
                break;
            }
            self.pos += 1;
            let rhs = self.binary(prec + 1, r)?;
            lhs = fold(Expr::Binary(op, Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }

    fn unary(&mut self, r: &mut dyn Resolver) -> Result<Expr, ParseError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return self.error("expression is too complex");
        }
        let op = match self.peek().map(|t| &t.tok) {
            Some(Tok::Punct(Punct::Minus)) => Some(UnOp::Neg),
            Some(Tok::Punct(Punct::Tilde)) => Some(UnOp::Not),
            Some(Tok::Punct(Punct::Bang)) => Some(UnOp::LNot),
            Some(Tok::Punct(Punct::Plus)) => {
                self.pos += 1;
                return self.unary(r);
            }
            _ => None,
        };
        if let Some(op) = op {
            self.pos += 1;
            let e = self.unary(r)?;
            return Ok(fold(Expr::Unary(op, Box::new(e))));
        }
        self.primary(r)
    }

    fn primary(&mut self, r: &mut dyn Resolver) -> Result<Expr, ParseError> {
        let Some(t) = self.peek() else {
            return self.error("expected an expression");
        };
        let col = t.col;
        let wrap = |res: Result<Expr, String>| res.map_err(|msg| ParseError { col, msg });
        match &t.tok {
            Tok::Int(v) => {
                self.pos += 1;
                Ok(Expr::Num(*v))
            }
            Tok::Ident(name) => {
                self.pos += 1;
                wrap(r.ident(name))
            }
            Tok::NumLabel { n, forward } => {
                self.pos += 1;
                wrap(r.num_label(*n, *forward))
            }
            Tok::Punct(Punct::LParen) => {
                self.pos += 1;
                let e = self.expr(r)?;
                self.expect(Punct::RParen)?;
                Ok(e)
            }
            Tok::Punct(Punct::Percent) => self.error("relocation operator not allowed here"),
            Tok::BigInt(_) => self.error("integer literal too large (more than 64 bits)"),
            other => self.error(format!("expected an expression, found {other}")),
        }
    }
}

/// Folds operations on literal numbers so that constant operands stay simple.
fn fold(e: Expr) -> Expr {
    struct NoEnv;
    impl Env for NoEnv {
        fn symbol(&self, _: SymId, _: u32) -> Result<Option<Value>, String> {
            Ok(None)
        }
        fn section_addr(&self, _: SecRef) -> Option<i64> {
            None
        }
    }
    if e.is_constant() {
        if let Ok(Some(Value::Abs(v))) = eval(&e, &NoEnv, 0) {
            return Expr::Num(v);
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use std::collections::HashMap;

    /// Test resolver: names map to symbols; symbols are labels in section 0
    /// or absolute values.
    struct Syms {
        names: Vec<String>,
        values: HashMap<String, Value>,
    }

    impl Resolver for Syms {
        fn ident(&mut self, name: &str) -> Result<Expr, String> {
            if name == "." {
                return Ok(Expr::Loc(SecRef { obj: 0, sec: 0 }, 100));
            }
            let id = match self.names.iter().position(|n| n == name) {
                Some(i) => i,
                None => {
                    self.names.push(name.to_string());
                    self.names.len() - 1
                }
            };
            Ok(Expr::Sym(id as u32))
        }
        fn num_label(&mut self, _: u32, _: bool) -> Result<Expr, String> {
            Err("no numeric labels".to_string())
        }
    }

    impl Env for Syms {
        fn symbol(&self, id: SymId, _: u32) -> Result<Option<Value>, String> {
            Ok(self.values.get(&self.names[id as usize]).copied())
        }
        fn section_addr(&self, sec: SecRef) -> Option<i64> {
            (sec.sec == 1).then_some(0x1000)
        }
    }

    fn syms() -> Syms {
        let s0 = SecRef { obj: 0, sec: 0 };
        let s1 = SecRef { obj: 0, sec: 1 };
        let mut values = HashMap::new();
        values.insert("a".to_string(), Value::Rel(s0, 8));
        values.insert("b".to_string(), Value::Rel(s0, 20));
        values.insert("c".to_string(), Value::Rel(s1, 4));
        values.insert("k".to_string(), Value::Abs(3));
        Syms {
            names: Vec::new(),
            values,
        }
    }

    fn ev(text: &str) -> Result<Option<Value>, String> {
        let toks = tokenize(text).unwrap();
        let mut s = syms();
        let mut p = Parser::new(&toks, text.len() as u32);
        let e = p.expr(&mut s).map_err(|e| e.msg)?;
        assert!(p.at_end(), "trailing tokens in {text}");
        eval(&e, &s, 0)
    }

    fn abs(text: &str) -> i64 {
        match ev(text) {
            Ok(Some(Value::Abs(v))) => v,
            other => panic!("{text}: {other:?}"),
        }
    }

    #[test]
    fn c_precedence() {
        assert_eq!(abs("1 + 2 * 3 << 1"), 14);
        assert_eq!(abs("1 | 2 ^ 3 & 1"), 3);
        assert_eq!(abs("-(3 + 4) * 2"), -14);
        assert_eq!(abs("1 < 2 == 1"), 0); // (1<2) is -1
        assert_eq!(abs("2 + 3 > 4 && 0 || 7"), 1);
        assert_eq!(abs("~0 & 0xff"), 255);
        assert_eq!(abs("!3 + !0"), 1);
        assert_eq!(abs("+5"), 5);
    }

    #[test]
    fn arithmetic_semantics() {
        assert_eq!(abs("7 / 2"), 3);
        assert_eq!(abs("-7 / 2"), -3);
        assert_eq!(abs("-7 % 2"), -1);
        assert_eq!(abs("1 == 1"), -1);
        assert_eq!(abs("2 < 1"), 0);
        assert_eq!(abs("-1 >> 60"), 15);
        assert_eq!(abs("1 << 64"), 0);
        assert_eq!(ev("1 / 0"), Err("division by zero".to_string()));
        assert_eq!(ev("5 % (2 - 2)"), Err("division by zero".to_string()));
    }

    #[test]
    fn relocatable_values() {
        let s0 = SecRef { obj: 0, sec: 0 };
        assert_eq!(ev("a + 4"), Ok(Some(Value::Rel(s0, 12))));
        assert_eq!(ev("4 + a - 1"), Ok(Some(Value::Rel(s0, 11))));
        assert_eq!(abs("b - a"), 12);
        assert_eq!(abs("(b - a) * k"), 36);
        assert_eq!(abs(". - a"), 92);
        assert!(ev("a + b").is_err());
        assert!(ev("a * 2").is_err());
        assert!(ev("-a").is_err());
        assert!(ev("k - a").is_err());
        // Different sections: known only when both are placed.
        assert_eq!(ev("c - a"), Ok(None));
        assert_eq!(ev("undefined_sym + 1"), Ok(None));
    }

    #[test]
    fn parse_errors() {
        assert!(ev("1 +").is_err());
        assert!(ev("(1").is_err());
        assert!(ev("%hi(x)").is_err());
        assert!(ev(")").is_err());
    }
}
