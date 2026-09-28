//! The inspection model: how the objects the debugger shows relate to each
//! other, from source lines down to netlist cells.
//!
//! * Module instances ([`ModuleInfo`]) come from the netlist hierarchy, which
//!   synthesis preserves; every cell belongs to exactly one instance.
//! * Pipeline stages ([`StageMap`]) are attributed to cells structurally:
//!   a stage's logic is the combinational fan-in cone of the registers that
//!   the next stage loads, plus those registers themselves.
//! * Activity ([`Activity`]) is measured by replaying a cycle window and
//!   counting value changes per net.
//! * Waveforms ([`capture`]) are captured by replay as well, so any net can
//!   be traced at any time without recording everything in advance.

use std::collections::{HashMap, VecDeque};
use std::io::Write;

use strata_gate::{Compiled, Driver};
use strata_netlist::{NetId, PortDir};

use crate::probes::Signal;
use crate::session::Session;

// ------------------------------------------------------------------ modules

#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub index: u32,
    pub path: String,
    pub module: String,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    /// Cells directly in this instance.
    pub cells: Vec<u32>,
    /// Totals including descendants.
    pub total_cells: usize,
    pub total_flops: usize,
    pub total_area: f64,
}

/// The module hierarchy with statistics.
pub fn modules(c: &Compiled) -> Vec<ModuleInfo> {
    let nl = &c.netlist;
    let is_flop: Vec<bool> = (0..nl.cells.len()).map(|i| c.defs[c.cell_def[i] as usize].is_sequential()).collect();
    let mut out: Vec<ModuleInfo> = nl
        .instances
        .iter()
        .enumerate()
        .map(|(i, inst)| ModuleInfo {
            index: i as u32,
            path: if inst.path.is_empty() { nl.top.clone() } else { inst.path.clone() },
            module: inst.module.clone(),
            parent: inst.parent,
            children: inst.children.clone(),
            cells: inst.cells.clone(),
            total_cells: 0,
            total_flops: 0,
            total_area: 0.0,
        })
        .collect();
    // Children always have larger indices than their parents.
    for i in (0..out.len()).rev() {
        let own_cells = out[i].cells.len();
        let own_flops = out[i].cells.iter().filter(|&&k| is_flop[k as usize]).count();
        let own_area: f64 = out[i].cells.iter().map(|&k| c.defs[c.cell_def[k as usize] as usize].area).sum();
        let (mut tc, mut tf, mut ta) = (own_cells, own_flops, own_area);
        for &ch in &out[i].children.clone() {
            tc += out[ch as usize].total_cells;
            tf += out[ch as usize].total_flops;
            ta += out[ch as usize].total_area;
        }
        out[i].total_cells = tc;
        out[i].total_flops = tf;
        out[i].total_area = ta;
    }
    out
}

// ------------------------------------------------------------------ cells

#[derive(Debug, Clone)]
pub struct PinInfo {
    pub pin: String,
    pub output: bool,
    pub net: NetId,
    pub net_name: String,
    pub value: bool,
}

#[derive(Debug, Clone)]
pub struct CellInfo {
    pub index: u32,
    pub name: String,
    pub cell_type: String,
    pub module_path: String,
    pub area: f64,
    pub sequential: bool,
    /// The Liberty function of each output.
    pub functions: Vec<(String, String)>,
    pub pins: Vec<PinInfo>,
    /// RTL source location recorded by synthesis, if any.
    pub src: Option<String>,
    /// For flip-flops: the stored bit.
    pub state: Option<bool>,
}

pub fn cell_info(s: &Session, cell: u32) -> CellInfo {
    let c = &*s.compiled;
    let nl = &c.netlist;
    let cl = &nl.cells[cell as usize];
    let def = &c.defs[c.cell_def[cell as usize] as usize];
    let pins = cl
        .pins
        .iter()
        .map(|p| PinInfo {
            pin: p.name.clone(),
            output: p.dir == PortDir::Output,
            net: p.net,
            net_name: nl.net_name(p.net),
            value: s.engine().net(p.net),
        })
        .collect();
    let state = c.flop_of_cell(cell).map(|f| s.engine().flop_state(f as usize));
    CellInfo {
        index: cell,
        name: cl.name.clone(),
        cell_type: cl.ty.clone(),
        module_path: nl.instances[cl.instance as usize].path.clone(),
        area: def.area,
        sequential: def.is_sequential(),
        functions: def.outputs.iter().map(|o| (o.name.clone(), o.function.clone())).collect(),
        pins,
        src: cl.src.clone().or_else(|| nearest_src(c, cell)),
        state,
    }
}

/// For cells without a source attribute (combinational cells from logic
/// optimization): the source of the nearest flip-flop reachable through
/// the cell's fan-out, which names the RTL register the cell computes.
fn nearest_src(c: &Compiled, cell: u32) -> Option<String> {
    let nl = &c.netlist;
    let mut queue: VecDeque<NetId> =
        nl.cells[cell as usize].pins.iter().filter(|p| p.dir == PortDir::Output).map(|p| p.net).collect();
    let mut seen = std::collections::HashSet::new();
    while let Some(n) = queue.pop_front() {
        if !seen.insert(n) || seen.len() > 400 {
            continue;
        }
        for &f in c.fanout_flops(n) {
            if let Some(src) = &nl.cells[c.flop_cell[f as usize] as usize].src {
                return Some(src.clone());
            }
        }
        for &k in c.fanout_nodes(n) {
            queue.push_back(c.node_out[k as usize]);
        }
    }
    None
}

/// The cell that drives a net, if any.
pub fn driver_cell(c: &Compiled, net: NetId) -> Option<u32> {
    match c.driver[net as usize] {
        Driver::Node(k) => Some(c.node_cell[k as usize]),
        Driver::Flop(f, _) => Some(c.flop_cell[f as usize]),
        _ => None,
    }
}

/// Cells that read a net.
pub fn reader_cells(c: &Compiled, net: NetId) -> Vec<u32> {
    let mut v: Vec<u32> = c.fanout_nodes(net).iter().map(|&k| c.node_cell[k as usize]).collect();
    v.extend(c.fanout_flops(net).iter().map(|&f| c.flop_cell[f as usize]));
    v.sort_unstable();
    v.dedup();
    v
}

// ------------------------------------------------------------------ pipeline stages

pub const STAGES: [&str; 5] = ["F", "D", "X", "M", "W"];

/// For every cell, the pipeline stages whose logic it is part of (bit i =
/// STAGES[i]). Cells outside the core have no stage.
#[derive(Debug, Clone)]
pub struct StageMap {
    pub stages: Vec<u8>,
}

impl StageMap {
    /// Attributes cells to stages by the registers they feed:
    ///   F: the fetch unit (its registers and their input cones)
    ///   D: cones of the X-stage registers (decode, register read)
    ///   X: cones of the M-stage registers (ALU, branch, address, PMP)
    ///   M: cones of the W-stage registers and the data-bus command
    ///   W: cones of the register-file write port and the CSR unit
    pub fn build(c: &Compiled) -> StageMap {
        let nl = &c.netlist;
        let mut stages = vec![0u8; nl.cells.len()];
        let flop_name = |f: usize| -> String {
            let cell = &nl.cells[c.flop_cell[f] as usize];
            // The flop's Q net carries the RTL register name.
            cell.pins
                .iter()
                .find(|p| p.dir == PortDir::Output)
                .map(|p| nl.net_name(p.net))
                .unwrap_or_default()
        };
        let mut seeds: Vec<Vec<u32>> = vec![Vec::new(); 5];
        for f in 0..c.flops() {
            let name = flop_name(f);
            let inst = &nl.instances[nl.cells[c.flop_cell[f] as usize].instance as usize].path;
            let st = if inst.starts_with("u_core.u_fetch") {
                Some(0)
            } else if name.starts_with("u_core.x_") {
                Some(1)
            } else if name.starts_with("u_core.m_") {
                Some(2)
            } else if name.starts_with("u_core.w_") {
                Some(3)
            } else if inst.starts_with("u_core.u_rf") || inst.starts_with("u_core.u_csr") {
                Some(4)
            } else {
                None
            };
            if let Some(st) = st {
                seeds[st].push(f as u32);
            }
        }
        for (st, fl) in seeds.iter().enumerate() {
            let bit = 1u8 << st;
            let mut queue: VecDeque<NetId> = VecDeque::new();
            for &f in fl {
                stages[c.flop_cell[f as usize] as usize] |= bit;
                queue.extend(c.flop_inputs(f as usize).iter().copied());
            }
            // The data-bus command belongs to M; the fetch command to F.
            let extra = if st == 3 { "u_core.dbus_cmd" } else if st == 0 { "u_core.ibus_cmd" } else { "" };
            if !extra.is_empty() {
                for (i, n) in nl.nets.iter().enumerate() {
                    if n.names.iter().any(|x| x.starts_with(extra)) {
                        queue.push_back(i as NetId);
                    }
                }
            }
            let mut seen = vec![false; nl.nets.len()];
            while let Some(n) = queue.pop_front() {
                if std::mem::replace(&mut seen[n as usize], true) {
                    continue;
                }
                if let Driver::Node(k) = c.driver[n as usize] {
                    let cell = c.node_cell[k as usize] as usize;
                    stages[cell] |= bit;
                    queue.extend(c.node_inputs(k as usize).iter().copied());
                }
            }
        }
        // Stage cones are only meaningful inside the core.
        for (i, cell) in nl.cells.iter().enumerate() {
            if !nl.instances[cell.instance as usize].path.starts_with("u_core") {
                stages[i] = 0;
            }
        }
        StageMap { stages }
    }

    pub fn cells_of(&self, stage: usize) -> impl Iterator<Item = u32> + '_ {
        self.stages.iter().enumerate().filter(move |(_, s)| **s & (1 << stage) != 0).map(|(i, _)| i as u32)
    }
}

// ------------------------------------------------------------------ activity

/// Value changes per net over a window of cycles.
#[derive(Debug, Clone)]
pub struct Activity {
    pub start: u64,
    pub end: u64,
    pub net_toggles: Vec<u32>,
}

impl Activity {
    /// Toggles of a cell's outputs.
    pub fn cell(&self, c: &Compiled, cell: u32) -> u32 {
        c.netlist.cells[cell as usize]
            .pins
            .iter()
            .filter(|p| p.dir == PortDir::Output)
            .map(|p| self.net_toggles[p.net as usize])
            .sum()
    }
}

/// Measures activity over [start, end) by replay, then returns to the
/// current cycle.
pub fn activity(s: &mut Session, start: u64, end: u64) -> Activity {
    let here = s.cycle();
    s.goto(start);
    s.machine.engine.enable_toggle_counts(true);
    s.goto(end.max(start));
    let net_toggles = s.engine().toggle_counts().unwrap().to_vec();
    s.machine.engine.enable_toggle_counts(false);
    s.goto(here);
    Activity { start, end, net_toggles }
}

/// Nets whose value differs between two cycles (settled states).
pub fn changed_nets(s: &mut Session, a: u64, b: u64) -> Vec<NetId> {
    let here = s.cycle();
    s.goto(a);
    let va = s.engine().values().to_vec();
    s.goto(b);
    let vb = s.engine().values();
    let out = va.iter().zip(vb).enumerate().filter(|(_, (x, y))| x != y).map(|(i, _)| i as NetId).collect();
    s.goto(here);
    out
}

// ------------------------------------------------------------------ waveforms

/// Signal values for each cycle in [start, end).
#[derive(Debug, Clone)]
pub struct Waves {
    pub start: u64,
    pub signals: Vec<Signal>,
    /// values[cycle - start][signal]
    pub values: Vec<Vec<u64>>,
}

/// Captures signals over [start, end) by replay, then returns to the
/// current cycle.
pub fn capture(s: &mut Session, signals: &[Signal], start: u64, end: u64) -> Waves {
    let here = s.cycle();
    s.goto(start);
    let mut values = Vec::with_capacity((end - start) as usize);
    while s.cycle() < end {
        values.push(signals.iter().map(|sig| s.value(sig)).collect());
        s.goto(s.cycle() + 1);
    }
    s.goto(here);
    Waves { start, signals: signals.to_vec(), values }
}

impl Waves {
    /// Writes a VCD file (one time unit per clock cycle, with the clock).
    pub fn write_vcd(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "$date strata-debug $end")?;
        writeln!(w, "$timescale 20ns $end")?;
        writeln!(w, "$scope module strata_soc $end")?;
        writeln!(w, "$var wire 1 ! clk $end")?;
        let ids: Vec<String> = (0..self.signals.len()).map(vcd_id).collect();
        for (s, id) in self.signals.iter().zip(&ids) {
            let name = s.name.replace(['[', ']'], "_");
            writeln!(w, "$var wire {} {} {} $end", s.width(), id, name)?;
        }
        writeln!(w, "$upscope $end\n$enddefinitions $end")?;
        let mut prev: Vec<Option<u64>> = vec![None; self.signals.len()];
        for (k, row) in self.values.iter().enumerate() {
            let t = (self.start + k as u64) * 2;
            writeln!(w, "#{t}\n1!")?;
            for (i, &v) in row.iter().enumerate() {
                if prev[i] != Some(v) {
                    let width = self.signals[i].width();
                    let v = v >> self.signals[i].lsb;
                    if width == 1 {
                        writeln!(w, "{}{}", v & 1, ids[i])?;
                    } else {
                        writeln!(w, "b{:b} {}", v, ids[i])?;
                    }
                    prev[i] = Some(v << self.signals[i].lsb);
                }
            }
            writeln!(w, "#{}\n0!", t + 1)?;
        }
        Ok(())
    }
}

fn vcd_id(mut i: usize) -> String {
    let mut s = String::new();
    loop {
        s.push((b'"' + (i % 90) as u8) as char);
        i /= 90;
        if i == 0 {
            return s;
        }
    }
}

// ------------------------------------------------------------------ machine views

/// A region of the address map (docs/memory-map.md) by name.
pub fn region_of(addr: u32) -> &'static str {
    match addr {
        0x0000_0000..=0x0000_0FFF => "ROM",
        0x1000_0000..=0x1000_FFFF => "RAM",
        0x2000_0000..=0x2000_1FFF => "VRAM",
        0x2001_0000..=0x2001_0FFF => "FONT",
        0x4000_0000..=0x4000_0FFF => "SYS",
        0x4000_1000..=0x4000_1FFF => "UART",
        0x4000_2000..=0x4000_2FFF => "TIMER",
        0x4000_3000..=0x4000_3FFF => "GPIO",
        0x4000_4000..=0x4000_4FFF => "SPI",
        0x4000_5000..=0x4000_5FFF => "VIDEO",
        0x4000_6000..=0x4000_6FFF => "INTC",
        _ => "unmapped",
    }
}

/// Reads memory as the data bus would see it (memories only; device
/// registers are shown from their flip-flops instead). None if unmapped.
pub fn read_word(s: &Session, addr: u32) -> Option<u32> {
    let m = &s.machine.host.mem;
    let a = addr & !3;
    let (mem, base) = match region_of(a) {
        "ROM" => (&m.rom, 0x0000_0000),
        "RAM" => (&m.ram, 0x1000_0000),
        "VRAM" => (&m.vram, 0x2000_0000),
        "FONT" => (&m.font, 0x2001_0000),
        _ => return None,
    };
    mem.words.get(((a - base) / 4) as usize).copied()
}

/// Name of the program occupying the slot that contains `addr`, from the
/// program header at the slot's base (sw/include/strata/layout.h).
pub fn slot_program(s: &Session, addr: u32) -> Option<String> {
    const SLOTS: [(u32, u32); 3] = [(0x1000_8000, 0x3000), (0x1000_B000, 0x4000), (0x1000_F000, 0x1000)];
    let (base, _) = SLOTS.iter().find(|(b, n)| addr >= *b && addr < b + n)?;
    if read_word(s, *base)? != 0x474F_5250 {
        return None;
    }
    let mut name = Vec::new();
    for k in 0..4 {
        name.extend(read_word(s, base + 24 + 4 * k)?.to_le_bytes());
    }
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    Some(String::from_utf8_lossy(&name[..end]).into_owned())
}

/// Disassembly of the word at `pc` with symbolized targets.
pub fn disassemble(s: &Session, pc: u32, word: u32) -> String {
    let prog = slot_program(s, pc);
    strata_isa::disassemble_with(word, pc, &|t| {
        let img = s.debug.image_at(t, prog.as_deref())?;
        let (sym, off) = img.symbol_at(t)?;
        Some(if off == 0 { sym.name.clone() } else { format!("{}+{off}", sym.name) })
    })
}

/// Function name and offset for a PC.
pub fn symbolize(s: &Session, pc: u32) -> Option<String> {
    let prog = slot_program(s, pc);
    let img = s.debug.image_at(pc, prog.as_deref())?;
    let (sym, off) = img.symbol_at(pc)?;
    Some(if off == 0 { sym.name.clone() } else { format!("{}+{off}", sym.name) })
}

/// Pipeline occupancy at the current cycle: for each stage, (valid, pc,
/// instruction word if known).
pub fn pipeline(s: &Session) -> Vec<(&'static str, bool, u32, Option<u32>)> {
    s.probes
        .stages
        .iter()
        .map(|st| {
            let valid = s.value(&st.valid) != 0;
            let pc = s.value(&st.pc) as u32;
            let insn = st.insn.as_ref().map(|i| s.value(i) as u32);
            (st.name, valid, pc, insn)
        })
        .collect()
}

/// Register-file contents x0..x31, read from its flip-flops.
pub fn registers(s: &Session) -> [u32; 32] {
    let mut r = [0u32; 32];
    for (i, sig) in s.probes.regs.iter().enumerate() {
        if let Some(sig) = sig {
            r[i] = s.value(sig) as u32;
        }
    }
    r
}

/// Index of netlist cells by name, for lookups from the command line.
pub fn cell_index(c: &Compiled) -> HashMap<&str, u32> {
    c.netlist.cells.iter().enumerate().map(|(i, cell)| (cell.name.as_str(), i as u32)).collect()
}
