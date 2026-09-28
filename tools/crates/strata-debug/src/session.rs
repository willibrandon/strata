//! A debug session: the gate-level machine plus everything needed to move
//! through its execution deterministically.
//!
//! Determinism: the only inputs to the machine are the boot files and a
//! timeline of host events (terminal input, button changes, resets) and
//! fault injections, each stamped with the cycle at which it takes effect.
//! Re-running from cycle 0 with the same timeline reproduces every net value
//! in every cycle. Checkpoints (complete snapshots of the netlist state and
//! host state) make moving to any cycle fast: restore the nearest earlier
//! checkpoint and replay the timeline.
//!
//! Between steps the machine is settled at `cycle()`: all nets hold the
//! values of that cycle, before its clock edge.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use strata_gate::{Compiled, Force, GateSim, GateState, PowerOn};
use strata_netlist::NetId;
use strata_sim::machine::{HostSnapshot, FLASH_IMAGE_OFFSET};
use strata_sim::{Machine, MachineConfig, Retire, StepEvents};

use crate::debuginfo::{DebugInfo, Image};
use crate::probes::{ArchProbes, Signal};

// ------------------------------------------------------------------ configuration

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub netlist: PathBuf,
    pub liberty: PathBuf,
    /// Boot ROM image (hex words or raw bytes).
    pub rom: PathBuf,
    /// Boot flash contents from FLASH_IMAGE_OFFSET.
    pub flash: Option<PathBuf>,
    /// RAM preload (test-harness shortcut).
    pub ram: Option<PathBuf>,
    /// Debug information: (image name, .sdb path). User programs use their
    /// program name.
    pub debug: Vec<(String, PathBuf)>,
    pub source_root: PathBuf,
    /// Power-on flip-flop contents: None = zero, Some(seed) = pseudo-random.
    pub power_on_seed: Option<u64>,
    /// Cycles between automatic checkpoints.
    pub checkpoint_interval: u64,
}

// ------------------------------------------------------------------ timeline

/// Host-side input applied at the start of a cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Bytes typed on the terminal (queued for the UART receiver).
    Uart(Vec<u8>),
    /// New button levels.
    Buttons(u8),
    /// Assert the external reset for this many cycles.
    Reset(u64),
}

/// Which memory macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemId {
    Rom,
    Ram,
    Vram,
    Font,
}

/// A fault injection applied at the start of a cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FaultAction {
    /// Force a net to a value (stuck-at), or release it with `None`.
    Force { net: NetId, value: Option<bool> },
    /// Invert a flip-flop's stored bit (single-event upset).
    Flip { flop: u32 },
    /// Invert one bit of a memory word.
    MemFlip { mem: MemId, word: u32, bit: u8 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TimelineEvent {
    Input(InputEvent),
    Fault(FaultAction),
}

// ------------------------------------------------------------------ history

/// A bus transaction observed at the CPU's bus ports.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BusTx {
    pub cycle: u64,
    pub data_bus: bool,
    pub addr: u32,
    pub write: bool,
    pub be: u8,
    pub wdata: u32,
    /// Filled in when the response arrives: (cycle, rdata, error).
    pub response: Option<(u64, u32, bool)>,
}

/// Something notable that happened (shown on the timeline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Trap { pc: u32, cause: u8, interrupt: bool },
    Trigger { id: u32 },
    Halt { code: u8 },
    Frame,
    Fault { description: String },
}

// ------------------------------------------------------------------ triggers

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Ge,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TriggerKind {
    /// The instruction at this address is about to complete (it is in W and
    /// finishes this cycle): the state shown is before its effects.
    Commit(u32),
    /// An instruction at one of these addresses is about to complete (a
    /// source line breakpoint).
    CommitAny(Vec<u32>),
    /// A data-bus access overlapping [lo, hi).
    Access { lo: u32, hi: u32, read: bool, write: bool },
    /// A signal compared with a value.
    Signal { signal: Signal, op: CmpOp, value: u64 },
    /// A signal changes value.
    Change { signal: Signal },
    /// A trap is taken (optionally a specific cause / interrupt or not).
    Trap { cause: Option<u8>, interrupt: Option<bool> },
    /// The machine reaches this cycle.
    Cycle(u64),
    /// The terminal output ends with this text.
    Uart(String),
    /// The machine halts.
    Halt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    pub id: u32,
    pub kind: TriggerKind,
    pub enabled: bool,
    pub hits: u64,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    Trigger(u32),
    Halted,
    Budget,
    Stepped,
}

// ------------------------------------------------------------------ checkpoints

struct Checkpoint {
    cycle: u64,
    gate: GateState,
    host: HostSnapshot,
}

/// Session contents that can be saved and replayed (the boot files are
/// referenced by path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFile {
    pub config: SessionConfig,
    pub timeline: Vec<(u64, TimelineEvent)>,
    pub triggers: Vec<Trigger>,
    pub cycle: u64,
}

// ------------------------------------------------------------------ session

pub struct Session {
    pub config: SessionConfig,
    pub compiled: Arc<Compiled>,
    pub machine: Machine<GateSim>,
    pub probes: ArchProbes,
    pub debug: DebugInfo,
    /// Events by cycle; several events may share a cycle.
    timeline: BTreeMap<u64, Vec<TimelineEvent>>,
    checkpoints: BTreeMap<u64, Checkpoint>,
    pub triggers: Vec<Trigger>,
    next_trigger: u32,
    pub retired: Vec<Retire>,
    pub bus: Vec<BusTx>,
    pub events: Vec<(u64, Event)>,
    /// Record instruction fetches in `bus` (one per cycle; off by default).
    pub log_fetches: bool,
    /// The furthest cycle simulated on the current timeline.
    pub head: u64,
    /// Previous values of signals watched by Change triggers.
    watch: Vec<u64>,
    dpending: Option<usize>,
    ipending: Option<usize>,
    replaying: bool,
    /// Cap on retained history entries (oldest dropped first).
    pub history_limit: usize,
}

fn read_image(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if path.extension().is_some_and(|e| e == "hex") {
        let text = String::from_utf8_lossy(&bytes);
        let mut out = Vec::new();
        for tok in text.split_whitespace() {
            let w = u32::from_str_radix(tok, 16).map_err(|_| format!("{}: bad hex word {tok}", path.display()))?;
            out.extend_from_slice(&w.to_le_bytes());
        }
        Ok(out)
    } else {
        Ok(bytes)
    }
}

impl Session {
    pub fn new(config: SessionConfig) -> Result<Session, String> {
        let compiled = strata_gate::load(&config.netlist, &config.liberty, "strata_soc")?;
        Self::with_compiled(config, compiled)
    }

    /// Builds a session on an already compiled netlist (shared, immutable).
    pub fn with_compiled(config: SessionConfig, compiled: Arc<Compiled>) -> Result<Session, String> {
        let power_on = config.power_on_seed.map_or(PowerOn::Zero, PowerOn::Random);
        let engine = GateSim::new(Arc::clone(&compiled), power_on)?;
        let mut machine = Machine::new(engine, MachineConfig::default());
        machine.host.mem.rom.load_bytes(&read_image(&config.rom)?);
        if let Some(p) = &config.ram {
            machine.host.mem.ram.load_bytes(&read_image(p)?);
        }
        if let Some(p) = &config.flash {
            machine.host.flash.program(FLASH_IMAGE_OFFSET, &read_image(p)?);
        }
        let probes = ArchProbes::resolve(&compiled.netlist);
        let mut debug = DebugInfo::default();
        for (name, path) in &config.debug {
            debug.add(Image::load(name, path, &config.source_root)?);
        }
        let mut s = Session {
            config,
            compiled,
            machine,
            probes,
            debug,
            timeline: BTreeMap::new(),
            checkpoints: BTreeMap::new(),
            triggers: Vec::new(),
            next_trigger: 1,
            retired: Vec::new(),
            bus: Vec::new(),
            events: Vec::new(),
            log_fetches: false,
            head: 0,
            watch: Vec::new(),
            dpending: None,
            ipending: None,
            replaying: false,
            history_limit: 2_000_000,
        };
        s.machine.outputs();
        s.save_checkpoint();
        Ok(s)
    }

    pub fn cycle(&self) -> u64 {
        self.machine.cycle()
    }

    pub fn engine(&self) -> &GateSim {
        &self.machine.engine
    }

    /// Current value of a signal, positioned at its RTL bit index.
    pub fn value(&self, s: &Signal) -> u64 {
        self.machine.engine.bus(&s.bits) << s.lsb
    }

    // -------------------------------------------------------------- checkpoints

    fn save_checkpoint(&mut self) {
        let c = self.cycle();
        self.checkpoints.insert(c, Checkpoint { cycle: c, gate: self.machine.engine.save(), host: self.machine.host_snapshot() });
    }

    /// Cycles at which checkpoints exist.
    pub fn checkpoint_cycles(&self) -> Vec<u64> {
        self.checkpoints.keys().copied().collect()
    }

    /// Takes a checkpoint at the current cycle now.
    pub fn checkpoint_now(&mut self) {
        self.save_checkpoint();
    }

    fn restore_checkpoint(&mut self, cycle: u64) {
        let cp = &self.checkpoints[&cycle];
        self.machine.engine.restore(&cp.gate);
        self.machine.restore_host(&cp.host);
        debug_assert_eq!(cp.cycle, self.machine.cycle());
        self.machine.outputs();
    }

    // -------------------------------------------------------------- timeline

    /// Records an event at the current cycle and applies it. If the session
    /// is not at the head of its timeline, the future is discarded first:
    /// the timeline branches here.
    pub fn record(&mut self, ev: TimelineEvent) {
        let now = self.cycle();
        if now < self.head {
            self.truncate_future();
        }
        self.timeline.entry(now).or_default().push(ev.clone());
        self.apply(&ev);
        self.machine.resettle();
        // A checkpoint at this cycle predates the event: replace it.
        self.save_checkpoint();
    }

    /// Schedules a fault at a future cycle (the timeline does not branch).
    pub fn schedule_fault(&mut self, cycle: u64, action: FaultAction) {
        self.timeline.entry(cycle).or_default().push(TimelineEvent::Fault(action));
    }

    pub fn timeline(&self) -> Vec<(u64, TimelineEvent)> {
        self.timeline.iter().flat_map(|(c, v)| v.iter().map(move |e| (*c, e.clone()))).collect()
    }

    fn truncate_future(&mut self) {
        let now = self.cycle();
        self.timeline.retain(|&c, _| c <= now);
        self.checkpoints.retain(|&c, _| c <= now);
        self.retired.retain(|r| r.cycle < now);
        self.bus.retain(|b| b.cycle < now);
        self.events.retain(|(c, _)| *c < now);
        self.dpending = None;
        self.ipending = None;
        self.head = now;
    }

    fn apply(&mut self, ev: &TimelineEvent) {
        let m = &mut self.machine;
        match ev {
            TimelineEvent::Input(InputEvent::Uart(b)) => m.host.uart.send(b),
            TimelineEvent::Input(InputEvent::Buttons(v)) => m.host.buttons = *v,
            TimelineEvent::Input(InputEvent::Reset(n)) => m.reset(*n),
            TimelineEvent::Fault(f) => {
                let desc = self.describe_fault(f);
                let m = &mut self.machine;
                match f {
                    FaultAction::Force { net, value } => {
                        let force = value.map(|v| if v { Force::One } else { Force::Zero });
                        let _ = m.engine.set_force(*net, force);
                    }
                    FaultAction::Flip { flop } => m.engine.flip_flop(*flop as usize),
                    FaultAction::MemFlip { mem, word, bit } => {
                        let sram = match mem {
                            MemId::Rom => &mut m.host.mem.rom,
                            MemId::Ram => &mut m.host.mem.ram,
                            MemId::Vram => &mut m.host.mem.vram,
                            MemId::Font => &mut m.host.mem.font,
                        };
                        let n = sram.words.len();
                        sram.words[*word as usize % n] ^= 1 << (bit & 31);
                    }
                }
                if !self.replaying {
                    let c = self.cycle();
                    self.events.push((c, Event::Fault { description: desc }));
                }
            }
        }
    }

    pub fn describe_fault(&self, f: &FaultAction) -> String {
        let nl = &self.compiled.netlist;
        match f {
            FaultAction::Force { net, value: Some(v) } => format!("net {} stuck at {}", nl.net_name(*net), *v as u8),
            FaultAction::Force { net, value: None } => format!("net {} released", nl.net_name(*net)),
            FaultAction::Flip { flop } => {
                let cell = self.compiled.flop_cell[*flop as usize];
                format!("upset in flip-flop {}", nl.cells[cell as usize].name)
            }
            FaultAction::MemFlip { mem, word, bit } => format!("bit {bit} of {mem:?} word {word} flipped"),
        }
    }

    // -------------------------------------------------------------- stepping

    /// Simulates one cycle: completes the current cycle, then applies the
    /// timeline events of the next one.
    fn advance(&mut self) -> StepEvents {
        let cycle = self.cycle();
        // History is recorded once, the first time a cycle is simulated.
        self.replaying = cycle < self.head;
        self.observe_bus(cycle);
        let ev = self.machine.step();
        let now = self.cycle();
        if let Some(evs) = self.timeline.get(&now).cloned() {
            for e in &evs {
                self.apply(e);
            }
            self.machine.resettle();
        }
        if !self.replaying {
            if let Some(r) = ev.retire {
                if r.trap {
                    self.events.push((r.cycle, Event::Trap { pc: r.pc, cause: r.cause, interrupt: r.intr }));
                }
                self.retired.push(r);
            }
            if ev.frame_done {
                self.events.push((cycle, Event::Frame));
            }
            if self.machine.halted() && !matches!(self.events.last(), Some((_, Event::Halt { .. }))) {
                self.events.push((cycle, Event::Halt { code: self.machine.last_out.halt_code }));
            }
            self.trim_history();
        }
        if now > self.head {
            self.head = now;
        }
        if now % self.config.checkpoint_interval == 0 && !self.checkpoints.contains_key(&now) {
            self.save_checkpoint();
        }
        ev
    }

    fn trim_history(&mut self) {
        let lim = self.history_limit;
        if self.retired.len() > lim {
            self.retired.drain(..lim / 2);
        }
        if self.bus.len() > lim {
            let drop = lim / 2;
            self.bus.drain(..drop);
            self.dpending = self.dpending.and_then(|i| i.checked_sub(drop));
            self.ipending = self.ipending.and_then(|i| i.checked_sub(drop));
        }
    }

    /// Records bus commands and responses of the current (settled) cycle.
    fn observe_bus(&mut self, cycle: u64) {
        if self.replaying {
            return;
        }
        let e = &self.machine.engine;
        let d = &self.probes.dbus;
        if e.bus(&d.rsp_valid.bits) != 0 {
            if let Some(i) = self.dpending.take() {
                self.bus[i].response = Some((cycle, e.bus(&d.rsp_rdata.bits) as u32, e.bus(&d.rsp_err.bits) != 0));
            }
        }
        if e.bus(&d.cmd_valid.bits) != 0 && e.bus(&d.cmd_ready.bits) != 0 {
            self.bus.push(BusTx {
                cycle,
                data_bus: true,
                addr: e.bus(&d.cmd_addr.bits) as u32,
                write: d.cmd_we.as_ref().is_some_and(|s| e.bus(&s.bits) != 0),
                be: d.cmd_be.as_ref().map_or(0xF, |s| e.bus(&s.bits) as u8),
                wdata: d.cmd_wdata.as_ref().map_or(0, |s| e.bus(&s.bits) as u32),
                response: None,
            });
            self.dpending = Some(self.bus.len() - 1);
        }
        if self.log_fetches {
            let i = &self.probes.ibus;
            if e.bus(&i.rsp_valid.bits) != 0 {
                if let Some(k) = self.ipending.take() {
                    self.bus[k].response = Some((cycle, e.bus(&i.rsp_rdata.bits) as u32, e.bus(&i.rsp_err.bits) != 0));
                }
            }
            if e.bus(&i.cmd_valid.bits) != 0 && e.bus(&i.cmd_ready.bits) != 0 {
                self.bus.push(BusTx {
                    cycle,
                    data_bus: false,
                    addr: e.bus(&i.cmd_addr.bits) as u32,
                    write: false,
                    be: 0xF,
                    wdata: 0,
                    response: None,
                });
                self.ipending = Some(self.bus.len() - 1);
            }
        }
    }

    /// Moves to `target` (earlier or later) by restoring the nearest
    /// earlier checkpoint and replaying the timeline. History for the
    /// replayed interval is rebuilt only when moving forward past the head.
    pub fn goto(&mut self, target: u64) {
        let now = self.cycle();
        let start = self
            .checkpoints
            .range(..=target)
            .next_back()
            .map(|(&c, _)| c)
            .unwrap_or(0);
        if target < now || start > now {
            self.restore_checkpoint(start);
        }
        while self.cycle() < target {
            self.advance();
        }
        self.replaying = false;
        self.reset_watch();
    }

    /// The last retirement at or before `cycle` that is in the history.
    pub fn retired_before(&self, cycle: u64) -> Option<&Retire> {
        let i = self.retired.partition_point(|r| r.cycle < cycle);
        self.retired[..i].last()
    }

    // -------------------------------------------------------------- triggers

    pub fn add_trigger(&mut self, kind: TriggerKind, label: impl Into<String>) -> u32 {
        let id = self.next_trigger;
        self.next_trigger += 1;
        self.triggers.push(Trigger { id, kind, enabled: true, hits: 0, label: label.into() });
        self.reset_watch();
        id
    }

    pub fn remove_trigger(&mut self, id: u32) {
        self.triggers.retain(|t| t.id != id);
        self.reset_watch();
    }

    fn reset_watch(&mut self) {
        self.watch = self
            .triggers
            .iter()
            .map(|t| match &t.kind {
                TriggerKind::Change { signal } => self.machine.engine.bus(&signal.bits),
                _ => 0,
            })
            .collect();
    }

    /// Checks triggers after a step; returns the first that fired.
    fn check_triggers(&mut self, ev: &StepEvents, bus_from: usize) -> Option<u32> {
        let mut fired = None;
        let cycle = self.cycle();
        let out = *self.machine.outputs();
        for (k, t) in self.triggers.iter_mut().enumerate() {
            if !t.enabled {
                continue;
            }
            let e = &self.machine.engine;
            let hit = match &t.kind {
                TriggerKind::Commit(pc) => out.trace.valid && !out.trace.trap && out.trace.pc == *pc,
                TriggerKind::CommitAny(pcs) => out.trace.valid && !out.trace.trap && pcs.contains(&out.trace.pc),
                TriggerKind::Access { lo, hi, read, write } => self.bus[bus_from..].iter().any(|b| {
                    b.data_bus && b.addr < *hi && b.addr.wrapping_add(4) > *lo && ((b.write && *write) || (!b.write && *read))
                }),
                TriggerKind::Signal { signal, op, value } => {
                    let v = e.bus(&signal.bits);
                    match op {
                        CmpOp::Eq => v == *value,
                        CmpOp::Ne => v != *value,
                        CmpOp::Lt => v < *value,
                        CmpOp::Ge => v >= *value,
                    }
                }
                TriggerKind::Change { signal } => {
                    let v = e.bus(&signal.bits);
                    let changed = v != self.watch[k];
                    self.watch[k] = v;
                    changed
                }
                TriggerKind::Trap { cause, interrupt } => ev.retire.is_some_and(|r| {
                    r.trap && cause.is_none_or(|c| c == r.cause) && interrupt.is_none_or(|i| i == r.intr)
                }),
                TriggerKind::Cycle(c) => cycle == *c,
                TriggerKind::Uart(text) => {
                    ev.uart_byte.is_some() && self.machine.host.uart.received.ends_with(text.as_bytes())
                }
                TriggerKind::Halt => self.machine.halted(),
            };
            if hit {
                t.hits += 1;
                if fired.is_none() {
                    fired = Some(t.id);
                }
            }
        }
        fired
    }

    /// Runs until a trigger fires, the machine halts, `max_cycles` pass, or
    /// `stop()` returns true (checked every 1024 cycles).
    pub fn run(&mut self, max_cycles: u64, stop: &mut dyn FnMut() -> bool) -> StopReason {
        let end = self.cycle() + max_cycles;
        while self.cycle() < end {
            let bus_from = self.bus.len();
            let ev = self.advance();
            if let Some(id) = self.check_triggers(&ev, bus_from) {
                let c = self.cycle();
                self.events.push((c, Event::Trigger { id }));
                return StopReason::Trigger(id);
            }
            if self.machine.halted() {
                return StopReason::Halted;
            }
            if self.cycle() % 1024 == 0 && stop() {
                return StopReason::Budget;
            }
        }
        StopReason::Budget
    }

    pub fn step_cycle(&mut self) -> StopReason {
        let bus_from = self.bus.len();
        let ev = self.advance();
        match self.check_triggers(&ev, bus_from) {
            Some(id) => StopReason::Trigger(id),
            None => StopReason::Stepped,
        }
    }

    /// Runs until the next instruction completes (retires or traps).
    pub fn step_instruction(&mut self, max_cycles: u64) -> StopReason {
        let end = self.cycle() + max_cycles;
        while self.cycle() < end {
            let ev = self.advance();
            if ev.retire.is_some() || self.machine.halted() {
                return StopReason::Stepped;
            }
        }
        StopReason::Budget
    }

    /// Goes back to the cycle in which the previous instruction completed
    /// (settled before its effects).
    pub fn reverse_step_instruction(&mut self) -> bool {
        let now = self.cycle();
        match self.retired_before(now).map(|r| r.cycle) {
            Some(c) => {
                self.goto(c);
                true
            }
            None => false,
        }
    }

    // -------------------------------------------------------------- persistence

    pub fn to_file(&self) -> SessionFile {
        SessionFile {
            config: self.config.clone(),
            timeline: self.timeline(),
            triggers: self.triggers.clone(),
            cycle: self.cycle(),
        }
    }

    /// Recreates a saved session: same boot files and timeline, positioned at
    /// the saved cycle.
    pub fn from_file(f: SessionFile, compiled: Option<Arc<Compiled>>) -> Result<Session, String> {
        let mut s = match compiled {
            Some(c) => Session::with_compiled(f.config, c)?,
            None => Session::new(f.config)?,
        };
        for (c, e) in f.timeline {
            s.timeline.entry(c).or_default().push(e);
        }
        s.next_trigger = f.triggers.iter().map(|t| t.id + 1).max().unwrap_or(1);
        s.triggers = f.triggers;
        let enabled: Vec<bool> = s.triggers.iter().map(|t| t.enabled).collect();
        for t in &mut s.triggers {
            t.enabled = false;
        }
        s.goto(f.cycle);
        for (t, e) in s.triggers.iter_mut().zip(enabled) {
            t.enabled = e;
        }
        Ok(s)
    }
}
