//! Runs two engines on identical inputs and compares all of their outputs
//! every cycle. Used to show that the gate-level netlist behaves exactly like
//! the RTL on whole programs.

use crate::engine::Engine;
use crate::ports::{SocIn, SocOut};

#[derive(Debug, Clone)]
pub struct Mismatch {
    pub cycle: u64,
    pub a: SocOut,
    pub b: SocOut,
}

impl Mismatch {
    /// The differing output fields, as `name: a vs b` lines.
    pub fn describe(&self) -> String {
        let a = format!("{:#?}", self.a);
        let b = format!("{:#?}", self.b);
        let mut out = format!("cycle {}: outputs differ\n", self.cycle);
        for (la, lb) in a.lines().zip(b.lines()) {
            if la != lb {
                out += &format!("  A {}\n  B {}\n", la.trim(), lb.trim());
            }
        }
        out
    }
}

pub struct Lockstep<A: Engine, B: Engine> {
    pub a: A,
    pub b: B,
    pub cycle: u64,
    /// The first mismatch, if any. Comparison continues to be performed but
    /// only the first is kept.
    pub mismatch: Option<Mismatch>,
    /// Cycles to skip before comparing (power-on state before reset is
    /// allowed to differ).
    pub skip: u64,
}

impl<A: Engine, B: Engine> Lockstep<A, B> {
    pub fn new(a: A, b: B, skip: u64) -> Self {
        Lockstep { a, b, cycle: 0, mismatch: None, skip }
    }
}

impl<A: Engine, B: Engine> Engine for Lockstep<A, B> {
    fn describe(&self) -> String {
        format!("lockstep [A: {}] [B: {}]", self.a.describe(), self.b.describe())
    }

    fn set_inputs(&mut self, i: &SocIn) {
        self.a.set_inputs(i);
        self.b.set_inputs(i);
    }

    fn eval(&mut self) {
        self.a.eval();
        self.b.eval();
    }

    fn outputs(&self) -> SocOut {
        self.a.outputs()
    }

    fn tick(&mut self) {
        if self.mismatch.is_none() && self.cycle >= self.skip {
            let (oa, ob) = (self.a.outputs(), self.b.outputs());
            if oa != ob {
                self.mismatch = Some(Mismatch { cycle: self.cycle, a: oa, b: ob });
            }
        }
        self.a.tick();
        self.b.tick();
        self.cycle += 1;
    }
}
