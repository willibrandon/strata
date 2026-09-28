//! Cycle-based simulation of the Strata machine.
//!
//! A [`Machine`] couples a logic [`Engine`] implementing `strata_soc` with the
//! memory macros and the pin-level devices on the board. The machine owns no
//! behavior of its own: every instruction executed, every bus transaction and
//! every pixel comes from the engine's evaluation of the design.

pub mod devices;
pub mod engine;
pub mod lockstep;
pub mod machine;
pub mod mem;
pub mod ports;
pub mod retire;
pub mod script;

pub use engine::Engine;
pub use lockstep::{Lockstep, Mismatch};
pub use machine::{Machine, MachineConfig, StepEvents};
pub use mem::{MemAccess, Memories, Sram};
pub use ports::{MemPort, SocIn, SocOut, TracePort};
pub use retire::Retire;
