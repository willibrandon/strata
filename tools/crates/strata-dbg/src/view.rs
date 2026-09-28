//! What the interface shows for one frame, copied out of the session while
//! the lock is held briefly.

use std::sync::Arc;

use strata_debug::{inspect, BusTx, Event, Loc, Session, TimelineEvent, Trigger};
use strata_sim::Retire;

#[derive(Debug, Clone)]
pub struct StageView {
    pub name: &'static str,
    pub valid: bool,
    pub pc: u32,
    pub insn: Option<u32>,
    pub text: String,
    pub symbol: Option<String>,
    pub source: Option<Loc>,
}

#[derive(Default)]
pub struct View {
    pub cycle: u64,
    pub head: u64,
    pub halted: bool,
    pub halt_code: u8,
    pub regs: [u32; 32],
    pub mode: u64,
    pub csrs: Vec<(&'static str, u64)>,
    pub control: Vec<(&'static str, bool)>,
    pub stages: Vec<StageView>,
    pub retired: Vec<Retire>,
    /// Instructions retired since reset (minstret).
    pub instret: u64,
    pub bus: Vec<BusTx>,
    pub events: Vec<(u64, Event)>,
    pub uart: Vec<u8>,
    pub leds: u8,
    pub buttons: u8,
    pub triggers: Vec<Trigger>,
    pub timeline: Vec<(u64, TimelineEvent)>,
    pub checkpoints: Vec<u64>,
    pub display_frames: u64,
    pub display: Option<Arc<Vec<u8>>>,
    /// Net values (for coloring the scene and pin values).
    pub nets: Arc<Vec<u8>>,
    /// Toggle counts per net since the session started.
    pub toggles: Option<Arc<Vec<u32>>>,
    /// Nets that changed in the last cycle.
    pub changed: Arc<Vec<u32>>,
}

/// Source location of an instruction address: the C line when the code was
/// compiled with line information, otherwise the assembly source line.
pub fn source_of(s: &Session, pc: u32) -> Option<Loc> {
    let prog = inspect::slot_program(s, pc);
    let img = s.debug.image_at(pc, prog.as_deref())?;
    img.source_line(pc).or_else(|| img.asm_line(pc))
}

impl View {
    pub fn gather(s: &mut Session, prev: &View) -> View {
        let pipeline = inspect::pipeline(s);
        let stages = pipeline
            .iter()
            .map(|&(name, valid, pc, insn)| {
                let word = insn.or_else(|| inspect::read_word(s, pc));
                StageView {
                    name,
                    valid,
                    pc,
                    insn,
                    text: word.map(|w| inspect::disassemble(s, pc, w)).unwrap_or_default(),
                    symbol: inspect::symbolize(s, pc),
                    source: source_of(s, pc),
                }
            })
            .collect();
        let tail = |n: usize, len: usize| len.saturating_sub(n);
        let now = s.cycle();
        let retired_end = s.retired.partition_point(|r| r.cycle < now);
        let bus_end = s.bus.partition_point(|b| b.cycle < now);
        let frames = s.machine.host.display.frames;
        let display = if frames != prev.display_frames || prev.display.is_none() {
            Some(Arc::new(s.machine.host.display.rgb8()))
        } else {
            prev.display.clone()
        };
        View {
            cycle: now,
            head: s.head,
            halted: s.machine.halted(),
            halt_code: s.machine.last_out.halt_code,
            regs: inspect::registers(s),
            mode: s.value(&s.probes.mode),
            csrs: s.probes.csr.iter().map(|(l, sig)| (*l, s.value(sig))).collect(),
            control: s.probes.stall.iter().map(|(l, sig)| (*l, s.value(sig) != 0)).collect(),
            stages,
            retired: s.retired[tail(400, retired_end)..retired_end].to_vec(),
            instret: s.probes.csr.iter().find(|(l, _)| *l == "minstret").map_or(0, |(_, sig)| s.value(sig)),
            bus: s.bus[tail(400, bus_end)..bus_end].to_vec(),
            events: s.events.iter().filter(|(c, _)| *c <= now).cloned().collect(),
            uart: s.machine.host.uart.received.clone(),
            leds: s.machine.last_out.led,
            buttons: s.machine.host.buttons,
            triggers: s.triggers.clone(),
            timeline: s.timeline(),
            checkpoints: s.checkpoint_cycles(),
            display_frames: frames,
            display,
            nets: Arc::new(s.engine().values().to_vec()),
            toggles: s.engine().toggle_counts().map(|t| Arc::new(t.to_vec())),
            changed: Arc::new(s.engine().last_changes().to_vec()),
        }
    }
}
