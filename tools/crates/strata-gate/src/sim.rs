//! The gate-level simulator: net values, flip-flop states, event-driven
//! evaluation, fault injection, and snapshots.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use strata_netlist::{NetId, NET_CONST0, NET_CONST1};
use strata_sim::ports::{output_setters, SocIn, SocOut, SOC_INPUTS, SOC_OUTPUTS};
use strata_sim::Engine;

use crate::compile::{Compiled, Driver};

/// How combinational logic is evaluated. Both give identical results;
/// `Full` exists to check that claim and as a performance baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvalMode {
    /// Only re-evaluate nodes whose inputs changed.
    Event,
    /// Evaluate every node, every cycle.
    Full,
}

/// Initial contents of flip-flops at power-on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerOn {
    Zero,
    /// Pseudo-random bits from a seed (checks that the design does not depend
    /// on power-on state).
    Random(u64),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub cycles: u64,
    pub node_evals: u64,
    pub flop_evals: u64,
    pub net_changes: u64,
}

/// Complete dynamic state, for checkpoints.
#[derive(Clone, Serialize, Deserialize)]
pub struct GateState {
    pub val: Vec<u8>,
    pub state: Vec<u8>,
    pub dirty: Vec<u64>,
    pub active: Vec<u64>,
    pub force: Vec<u8>,
}

/// A forced value on a net (fault injection). Forces override the net's
/// driver until removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Force {
    Zero,
    One,
}

pub struct GateSim {
    pub c: Arc<Compiled>,
    val: Vec<u8>,
    state: Vec<u8>,
    dirty: Vec<u64>,
    active: Vec<u64>,
    force: Vec<u8>,
    toggles: Option<Vec<u32>>,
    changes: Vec<(u32, u8)>,
    /// Nets that changed since the last clock edge began (see `last_changes`).
    changed: Vec<NetId>,
    changed_mark: Vec<u32>,
    epoch: u32,
    pub mode: EvalMode,
    pub stats: Stats,
    /// Bits of each SOC_INPUTS / SOC_OUTPUTS port, in that order.
    in_bits: Vec<Vec<NetId>>,
    out_bits: Vec<Vec<NetId>>,
    setters: Vec<fn(&mut SocOut, u64)>,
}

/// Sets bits 0..n of a bitset and clears the rest.
fn fill_bits(v: &mut [u64], n: usize) {
    for (w, word) in v.iter_mut().enumerate() {
        let lo = w * 64;
        *word = if n >= lo + 64 { u64::MAX } else if n > lo { (1u64 << (n - lo)) - 1 } else { 0 };
    }
}

#[inline]
fn set_bit(v: &mut [u64], i: usize) {
    v[i >> 6] |= 1 << (i & 63);
}

impl GateSim {
    pub fn new(c: Arc<Compiled>, power_on: PowerOn) -> Result<GateSim, String> {
        let mut in_bits = Vec::new();
        for (name, width) in SOC_INPUTS {
            let p = c.input_port(name).ok_or_else(|| format!("netlist has no input port {name}"))?;
            let bits = c.inputs[p].bits.clone();
            if bits.len() != *width as usize {
                return Err(format!("input {name} is {} bits, expected {width}", bits.len()));
            }
            in_bits.push(bits);
        }
        let mut out_bits = Vec::new();
        for (name, width) in SOC_OUTPUTS {
            let p = c.output_port(name).ok_or_else(|| format!("netlist has no output port {name}"))?;
            let bits = c.outputs[p].bits.clone();
            if bits.len() != *width as usize {
                return Err(format!("output {name} is {} bits, expected {width}", bits.len()));
            }
            out_bits.push(bits);
        }
        let nnets = c.netlist.nets.len();
        let mut s = GateSim {
            val: vec![0; nnets],
            state: vec![0; c.flops()],
            dirty: vec![0; c.nodes().div_ceil(64)],
            active: vec![0; c.flops().div_ceil(64)],
            force: vec![0; nnets],
            toggles: None,
            changes: Vec::new(),
            changed: Vec::new(),
            changed_mark: vec![0; nnets],
            epoch: 1,
            mode: EvalMode::Event,
            stats: Stats::default(),
            in_bits,
            out_bits,
            setters: output_setters(),
            c,
        };
        s.power_on(power_on);
        Ok(s)
    }

    /// Resets every flip-flop to its power-on value and re-evaluates all logic.
    pub fn power_on(&mut self, p: PowerOn) {
        let mut rng = match p {
            PowerOn::Zero => 0,
            PowerOn::Random(seed) => seed | 1,
        };
        for s in self.state.iter_mut() {
            *s = if rng == 0 {
                0
            } else {
                // xorshift64
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng & 1) as u8
            };
        }
        self.val.iter_mut().for_each(|v| *v = 0);
        self.val[NET_CONST1 as usize] = 1;
        for f in 0..self.c.flops() {
            let st = self.state[f];
            for &(net, func) in self.c.flop_outputs(f) {
                self.val[net as usize] = (func >> st) & 1;
            }
        }
        let (nodes, flops) = (self.c.nodes(), self.c.flops());
        fill_bits(&mut self.dirty, nodes);
        fill_bits(&mut self.active, flops);
        self.eval();
    }

    // ------------------------------------------------------------ inspection

    pub fn net(&self, n: NetId) -> bool {
        self.val[n as usize] != 0
    }

    /// Value of a bus given its bits, bit 0 first.
    pub fn bus(&self, bits: &[NetId]) -> u64 {
        bits.iter().enumerate().fold(0, |acc, (i, &b)| acc | ((self.val[b as usize] as u64) << i))
    }

    pub fn flop_state(&self, f: usize) -> bool {
        self.state[f] != 0
    }

    pub fn values(&self) -> &[u8] {
        &self.val
    }

    /// Nets whose value changed from the previous cycle's settled state to
    /// the current one (the effects of the last clock edge, plus anything
    /// applied since, such as forced nets).
    pub fn last_changes(&self) -> &[NetId] {
        &self.changed
    }

    /// Starts counting value changes per net (for activity views).
    pub fn enable_toggle_counts(&mut self, on: bool) {
        self.toggles = on.then(|| vec![0; self.val.len()]);
    }

    pub fn toggle_counts(&self) -> Option<&[u32]> {
        self.toggles.as_deref()
    }

    pub fn clear_toggle_counts(&mut self) {
        if let Some(t) = &mut self.toggles {
            t.iter_mut().for_each(|x| *x = 0);
        }
    }

    // ------------------------------------------------------------ faults

    pub fn forced(&self, n: NetId) -> Option<Force> {
        match self.force[n as usize] {
            2 => Some(Force::Zero),
            3 => Some(Force::One),
            _ => None,
        }
    }

    /// Forces a net to a value (or releases it). Takes effect immediately:
    /// logic reading the net is re-evaluated at the next `eval`.
    pub fn set_force(&mut self, n: NetId, f: Option<Force>) -> Result<(), String> {
        if n == NET_CONST0 || n == NET_CONST1 {
            return Err("constant nets cannot be forced".into());
        }
        let i = n as usize;
        self.force[i] = match f {
            None => 0,
            Some(Force::Zero) => 2,
            Some(Force::One) => 3,
        };
        match (f, self.c.driver[i]) {
            (Some(v), _) => self.write_net(n, (v == Force::One) as u8),
            (None, Driver::Node(k)) => set_bit(&mut self.dirty, k as usize),
            (None, Driver::Flop(fl, oi)) => {
                let (net, func) = self.c.flop_outputs(fl as usize)[oi as usize];
                let v = (func >> self.state[fl as usize]) & 1;
                self.write_net(net, v);
            }
            // Inputs are re-applied by the next set_inputs.
            (None, _) => {}
        }
        Ok(())
    }

    pub fn clear_forces(&mut self) {
        let forced: Vec<NetId> = (0..self.force.len()).filter(|&i| self.force[i] != 0).map(|i| i as NetId).collect();
        for n in forced {
            let _ = self.set_force(n, None);
        }
    }

    /// Inverts the stored bit of flop `f` (a single-event upset). Its outputs
    /// change immediately and it reloads normally at the next clock edge.
    pub fn flip_flop(&mut self, f: usize) {
        self.state[f] ^= 1;
        self.update_flop_outputs(f);
        set_bit(&mut self.active, f);
    }

    /// Sets the stored bit of flop `f`.
    pub fn set_flop(&mut self, f: usize, v: bool) {
        if self.state[f] != v as u8 {
            self.flip_flop(f);
        }
    }

    // ------------------------------------------------------------ checkpoints

    pub fn save(&self) -> GateState {
        GateState {
            val: self.val.clone(),
            state: self.state.clone(),
            dirty: self.dirty.clone(),
            active: self.active.clone(),
            force: self.force.clone(),
        }
    }

    pub fn restore(&mut self, s: &GateState) {
        self.val.copy_from_slice(&s.val);
        self.state.copy_from_slice(&s.state);
        self.dirty.copy_from_slice(&s.dirty);
        self.active.copy_from_slice(&s.active);
        self.force.copy_from_slice(&s.force);
    }

    // ------------------------------------------------------------ evaluation

    /// Writes a net value (respecting forces) and schedules its readers.
    #[inline]
    fn write_net(&mut self, n: NetId, mut v: u8) {
        let i = n as usize;
        let f = self.force[i];
        if f != 0 {
            v = f & 1;
        }
        if self.val[i] != v {
            self.val[i] = v;
            self.stats.net_changes += 1;
            if self.changed_mark[i] != self.epoch {
                self.changed_mark[i] = self.epoch;
                self.changed.push(n);
            }
            if let Some(t) = &mut self.toggles {
                t[i] = t[i].wrapping_add(1);
            }
            let c = &*self.c;
            for &k in &c.fan_node[c.fan_off[i] as usize..c.fan_off[i + 1] as usize] {
                self.dirty[(k >> 6) as usize] |= 1 << (k & 63);
            }
            for &k in &c.ffan_flop[c.ffan_off[i] as usize..c.ffan_off[i + 1] as usize] {
                self.active[(k >> 6) as usize] |= 1 << (k & 63);
            }
        }
    }

    #[inline]
    fn eval_node(&mut self, i: usize) {
        let c = &*self.c;
        let (a, b) = (c.node_in_off[i] as usize, c.node_in_off[i + 1] as usize);
        let mut idx = 0usize;
        for (k, &n) in c.node_in[a..b].iter().enumerate() {
            idx |= (self.val[n as usize] as usize) << k;
        }
        let v = if b - a <= 6 {
            ((c.node_tt[i] >> idx) & 1) as u8
        } else {
            let t = &c.node_big_tt[&(i as u32)];
            ((t[idx >> 6] >> (idx & 63)) & 1) as u8
        };
        let out = c.node_out[i];
        self.write_net(out, v);
    }

    fn update_flop_outputs(&mut self, f: usize) {
        let st = self.state[f];
        let (a, b) = (self.c.flop_out_off[f] as usize, self.c.flop_out_off[f + 1] as usize);
        for k in a..b {
            let (net, func) = self.c.flop_out[k];
            self.write_net(net, (func >> st) & 1);
        }
    }

    fn settle(&mut self) {
        match self.mode {
            EvalMode::Event => {
                let mut w = 0;
                let nw = self.dirty.len();
                while w < nw {
                    let bits = self.dirty[w];
                    if bits == 0 {
                        w += 1;
                        continue;
                    }
                    let b = bits.trailing_zeros() as usize;
                    self.dirty[w] &= !(1u64 << b);
                    self.eval_node((w << 6) | b);
                    self.stats.node_evals += 1;
                }
            }
            EvalMode::Full => {
                for i in 0..self.c.nodes() {
                    self.eval_node(i);
                }
                self.stats.node_evals += self.c.nodes() as u64;
                self.dirty.iter_mut().for_each(|w| *w = 0);
            }
        }
    }

    fn clock_edge(&mut self) {
        let c = Arc::clone(&self.c);
        self.changes.clear();
        self.changed.clear();
        self.epoch = self.epoch.wrapping_add(1).max(1);
        let full = self.mode == EvalMode::Full;
        for w in 0..self.active.len() {
            let mut bits = if full { u64::MAX } else { self.active[w] };
            while bits != 0 {
                let b = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let f = (w << 6) | b;
                if f >= c.flops() {
                    break;
                }
                let (a, e) = (c.flop_in_off[f] as usize, c.flop_in_off[f + 1] as usize);
                let mut idx = 0usize;
                for (k, &n) in c.flop_in[a..e].iter().enumerate() {
                    idx |= (self.val[n as usize] as usize) << k;
                }
                idx |= (self.state[f] as usize) << (e - a);
                let next = ((c.flop_tt[f] >> idx) & 1) as u8;
                self.stats.flop_evals += 1;
                if next != self.state[f] {
                    self.changes.push((f as u32, next));
                }
                if c.flop_idempotent[f] {
                    self.active[w] &= !(1u64 << b);
                }
            }
        }
        let changes = std::mem::take(&mut self.changes);
        for &(f, next) in &changes {
            self.state[f as usize] = next;
            self.update_flop_outputs(f as usize);
        }
        self.changes = changes;
        self.stats.cycles += 1;
    }
}

impl Engine for GateSim {
    fn describe(&self) -> String {
        format!(
            "gate: {} cells ({} flip-flops) mapped to {} library cell types",
            self.c.netlist.cells.len(),
            self.c.flops(),
            self.c.defs.len()
        )
    }

    fn set_inputs(&mut self, inputs: &SocIn) {
        let values = inputs.to_array();
        for (p, &v) in values.iter().enumerate() {
            for k in 0..self.in_bits[p].len() {
                let n = self.in_bits[p][k];
                self.write_net(n, ((v >> k) & 1) as u8);
            }
        }
    }

    fn eval(&mut self) {
        self.settle();
    }

    fn outputs(&self) -> SocOut {
        let mut o = SocOut::default();
        for (p, bits) in self.out_bits.iter().enumerate() {
            (self.setters[p])(&mut o, self.bus(bits));
        }
        o
    }

    fn tick(&mut self) {
        self.clock_edge();
    }
}
