//! Retirement records: one per instruction leaving the pipeline, read from
//! the design's trace outputs.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ports::TracePort;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retire {
    pub cycle: u64,
    pub pc: u32,
    pub insn: u32,
    pub next_pc: u32,
    pub mode: u8,
    /// The instruction trapped (exception or interrupt) instead of retiring.
    pub trap: bool,
    pub intr: bool,
    pub cause: u8,
    /// Register write: (rd, value).
    pub rd: Option<(u8, u32)>,
}

impl Retire {
    pub fn from_trace(cycle: u64, t: &TracePort) -> Option<Retire> {
        t.valid.then(|| Retire {
            cycle,
            pc: t.pc,
            insn: t.insn,
            next_pc: t.next_pc,
            mode: t.mode,
            trap: t.trap,
            intr: t.intr,
            cause: t.cause,
            rd: t.rd_we.then_some((t.rd, t.rd_wdata)),
        })
    }
}

/// The same text format as the iverilog testbench, so traces from different
/// simulators can be compared with `diff`.
impl fmt::Display for Retire {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:08x} {:08x}", self.cycle, self.pc, self.insn)?;
        if self.trap {
            write!(f, " trap {}{}", self.cause, if self.intr { "i" } else { "" })?;
        } else if let Some((rd, v)) = self.rd {
            write!(f, " x{rd}={v:08x}")?;
        }
        write!(f, " -> {:08x}", self.next_pc)
    }
}
