//! Liberty boolean functions (`function`, `next_state`, ...) compiled to
//! truth tables.
//!
//! Syntax: identifiers, constants `0`/`1`, prefix `!` and postfix `'` (not),
//! `^` (xor), `&`, `*` and juxtaposition (and), `|` and `+` (or), and
//! parentheses. Precedence from high to low: not, xor, and, or.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Const(bool),
    Var(String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Xor(Box<Expr>, Box<Expr>),
}

#[derive(Debug)]
pub struct FuncError(pub String);

impl fmt::Display for FuncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad boolean function: {}", self.0)
    }
}

impl std::error::Error for FuncError {}

#[derive(Debug, Clone, PartialEq)]
enum T {
    Id(String),
    Not,
    Tick,
    And,
    Or,
    Xor,
    LP,
    RP,
}

fn lex(s: &str) -> Result<Vec<T>, FuncError> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' => {
                i += 1;
                continue;
            }
            b'!' => out.push(T::Not),
            b'\'' => out.push(T::Tick),
            b'&' | b'*' => out.push(T::And),
            b'|' | b'+' => out.push(T::Or),
            b'^' => out.push(T::Xor),
            b'(' => out.push(T::LP),
            b')' => out.push(T::RP),
            c if c.is_ascii_alphanumeric() || c == b'_' || c == b'[' || c == b']' || c == b'.' => {
                let st = i;
                while i < b.len()
                    && (b[i].is_ascii_alphanumeric() || b"_[].".contains(&b[i]))
                {
                    i += 1;
                }
                out.push(T::Id(s[st..i].to_string()));
                continue;
            }
            _ => return Err(FuncError(format!("unexpected '{}' in \"{s}\"", c as char))),
        }
        i += 1;
    }
    Ok(out)
}

struct P {
    t: Vec<T>,
    i: usize,
}

impl P {
    fn peek(&self) -> Option<&T> {
        self.t.get(self.i)
    }

    fn or(&mut self) -> Result<Expr, FuncError> {
        let mut e = self.and()?;
        while self.peek() == Some(&T::Or) {
            self.i += 1;
            e = Expr::Or(Box::new(e), Box::new(self.and()?));
        }
        Ok(e)
    }

    fn and(&mut self) -> Result<Expr, FuncError> {
        let mut e = self.xor()?;
        loop {
            match self.peek() {
                Some(T::And) => {
                    self.i += 1;
                    e = Expr::And(Box::new(e), Box::new(self.xor()?));
                }
                // Juxtaposition is conjunction.
                Some(T::Id(_)) | Some(T::Not) | Some(T::LP) => {
                    e = Expr::And(Box::new(e), Box::new(self.xor()?));
                }
                _ => return Ok(e),
            }
        }
    }

    fn xor(&mut self) -> Result<Expr, FuncError> {
        let mut e = self.unary()?;
        while self.peek() == Some(&T::Xor) {
            self.i += 1;
            e = Expr::Xor(Box::new(e), Box::new(self.unary()?));
        }
        Ok(e)
    }

    fn unary(&mut self) -> Result<Expr, FuncError> {
        let mut e = match self.t.get(self.i).cloned() {
            Some(T::Not) => {
                self.i += 1;
                Expr::Not(Box::new(self.unary()?))
            }
            Some(T::LP) => {
                self.i += 1;
                let e = self.or()?;
                if self.peek() != Some(&T::RP) {
                    return Err(FuncError("missing ')'".into()));
                }
                self.i += 1;
                e
            }
            Some(T::Id(s)) => {
                self.i += 1;
                match s.as_str() {
                    "0" => Expr::Const(false),
                    "1" => Expr::Const(true),
                    _ => Expr::Var(s),
                }
            }
            other => return Err(FuncError(format!("unexpected {other:?}"))),
        };
        while self.peek() == Some(&T::Tick) {
            self.i += 1;
            e = Expr::Not(Box::new(e));
        }
        Ok(e)
    }
}

impl Expr {
    pub fn parse(s: &str) -> Result<Expr, FuncError> {
        let mut p = P { t: lex(s)?, i: 0 };
        let e = p.or()?;
        if p.i != p.t.len() {
            return Err(FuncError(format!("trailing input in \"{s}\"")));
        }
        Ok(e)
    }

    /// Variables referenced, in first-occurrence order.
    pub fn vars(&self) -> Vec<String> {
        fn walk(e: &Expr, out: &mut Vec<String>) {
            match e {
                Expr::Const(_) => {}
                Expr::Var(v) => {
                    if !out.contains(v) {
                        out.push(v.clone())
                    }
                }
                Expr::Not(a) => walk(a, out),
                Expr::And(a, b) | Expr::Or(a, b) | Expr::Xor(a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
            }
        }
        let mut v = Vec::new();
        walk(self, &mut v);
        v
    }

    pub fn eval(&self, lookup: &dyn Fn(&str) -> bool) -> bool {
        match self {
            Expr::Const(c) => *c,
            Expr::Var(v) => lookup(v),
            Expr::Not(a) => !a.eval(lookup),
            Expr::And(a, b) => a.eval(lookup) && b.eval(lookup),
            Expr::Or(a, b) => a.eval(lookup) || b.eval(lookup),
            Expr::Xor(a, b) => a.eval(lookup) ^ b.eval(lookup),
        }
    }

    /// Truth table over `inputs` (input `k` is bit `k` of the row index).
    /// Variables not in `inputs` are an error.
    pub fn truth_table(&self, inputs: &[String]) -> Result<TruthTable, FuncError> {
        for v in self.vars() {
            if !inputs.contains(&v) {
                return Err(FuncError(format!("variable {v} is not an input")));
            }
        }
        let n = inputs.len();
        if n > 16 {
            return Err(FuncError(format!("{n} inputs is too many")));
        }
        let rows = 1usize << n;
        let mut bits = vec![0u64; rows.div_ceil(64)];
        for r in 0..rows {
            let val = self.eval(&|name| {
                let k = inputs.iter().position(|i| i == name).unwrap();
                (r >> k) & 1 == 1
            });
            if val {
                bits[r / 64] |= 1 << (r % 64);
            }
        }
        Ok(TruthTable { inputs: n as u8, bits })
    }
}

/// A truth table: bit `r` of `bits` is the output for input row `r`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TruthTable {
    pub inputs: u8,
    pub bits: Vec<u64>,
}

impl TruthTable {
    pub fn get(&self, row: usize) -> bool {
        (self.bits[row / 64] >> (row % 64)) & 1 == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tt(f: &str, ins: &[&str]) -> Vec<bool> {
        let ins: Vec<String> = ins.iter().map(|s| s.to_string()).collect();
        let t = Expr::parse(f).unwrap().truth_table(&ins).unwrap();
        (0..1 << ins.len()).map(|r| t.get(r)).collect()
    }

    #[test]
    fn operators_and_precedence() {
        assert_eq!(tt("!A", &["A"]), [true, false]);
        assert_eq!(tt("A'", &["A"]), [true, false]);
        assert_eq!(tt("A&B", &["A", "B"]), [false, false, false, true]);
        assert_eq!(tt("A B", &["A", "B"]), [false, false, false, true]);
        assert_eq!(tt("A+B", &["A", "B"]), [false, true, true, true]);
        assert_eq!(tt("A^B", &["A", "B"]), [false, true, true, false]);
        // and binds tighter than or
        assert_eq!(tt("A|B&C", &["A", "B", "C"]), tt("A|(B&C)", &["A", "B", "C"]));
        // rows: D | DE<<1 | IQ<<2
        assert_eq!(tt("(D&DE) | (IQ&!DE)", &["D", "DE", "IQ"]),
                   [false, false, false, true, true, true, false, true]);
    }

    #[test]
    fn mux4() {
        let f = "(A0&!S0&!S1) | (A1&S0&!S1) | (A2&!S0&S1) | (A3&S0&S1)";
        let ins = ["A0", "A1", "A2", "A3", "S0", "S1"];
        let v = tt(f, &ins);
        for r in 0..64usize {
            let sel = (r >> 4) & 3;
            assert_eq!(v[r], (r >> sel) & 1 == 1);
        }
    }
}
