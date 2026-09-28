//! Debugging the Strata machine at the gate level.
//!
//! A [`Session`] runs the synthesized netlist with deterministic replay,
//! checkpoints, triggers and fault injection. The inspection model
//! ([`inspect`]) relates what the session shows at a cycle: source lines,
//! instructions, pipeline stages, bus transactions, RTL module instances and
//! netlist cells.

pub mod debuginfo;
pub mod inspect;
pub mod probes;
pub mod session;

pub use debuginfo::{DebugInfo, Image, Loc};
pub use probes::{ArchProbes, Signal};
pub use session::{
    BusTx, CmpOp, Event, FaultAction, InputEvent, MemId, Session, SessionConfig, SessionFile, StopReason,
    TimelineEvent, Trigger, TriggerKind,
};
