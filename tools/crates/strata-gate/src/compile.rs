//! Compilation of a flattened netlist and a cell library into the flat
//! structures the simulator evaluates.
//!
//! Each output of a combinational cell becomes a *node*: a truth table over
//! the cell's input nets. Nodes are sorted by logic level, so every node's
//! fan-out has a larger index than the node itself. Each flip-flop cell
//! becomes a *flop*: a next-state truth table over its data inputs and its
//! stored bit, plus the nets its outputs drive.

use std::collections::HashMap;
use std::fmt;

use strata_netlist::{CellDef, CellLibrary, NetId, Netlist, PortDir, NET_CONST0, NET_CONST1};

/// What drives a net.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Const(bool),
    /// Top-level input port bit: (port index in `Compiled::inputs`, bit).
    Input(u16, u8),
    Node(u32),
    /// Flop output: (flop index, output index within the cell).
    Flop(u32, u8),
    /// No driver (reported by `compile`).
    None,
}

#[derive(Debug, Clone)]
pub struct PortBinding {
    pub name: String,
    pub bits: Vec<NetId>,
}

/// The compiled, immutable form of a netlist; shared by all simulator states.
pub struct Compiled {
    pub netlist: Netlist,
    /// Library definitions of the cell types used, indexed by `cell_def`.
    pub defs: Vec<CellDef>,
    /// For each netlist cell, its index into `defs`.
    pub cell_def: Vec<u16>,

    // Nodes (combinational cell outputs), sorted by level.
    pub node_cell: Vec<u32>,
    pub node_output: Vec<u8>,
    pub node_out: Vec<NetId>,
    pub node_level: Vec<u32>,
    pub node_in_off: Vec<u32>,
    pub node_in: Vec<NetId>,
    /// Truth table for nodes with at most 6 inputs.
    pub node_tt: Vec<u64>,
    /// Truth tables for nodes with more than 6 inputs.
    pub node_big_tt: HashMap<u32, Vec<u64>>,
    pub max_level: u32,

    // Flops.
    pub flop_cell: Vec<u32>,
    pub flop_in_off: Vec<u32>,
    pub flop_in: Vec<NetId>,
    /// Next state over the data inputs followed by the stored bit (<= 6 vars).
    pub flop_tt: Vec<u64>,
    /// Output nets and their function of the stored bit: bit 0 = value when
    /// the state is 0, bit 1 = value when it is 1.
    pub flop_out_off: Vec<u32>,
    pub flop_out: Vec<(NetId, u8)>,
    /// Re-evaluating with unchanged inputs cannot change the state.
    pub flop_idempotent: Vec<bool>,

    // Fan-out of each net to nodes and to flops (CSR).
    pub fan_off: Vec<u32>,
    pub fan_node: Vec<u32>,
    pub ffan_off: Vec<u32>,
    pub ffan_flop: Vec<u32>,

    pub driver: Vec<Driver>,
    pub inputs: Vec<PortBinding>,
    pub outputs: Vec<PortBinding>,
    pub clock: NetId,
}

#[derive(Debug)]
pub struct CompileError(pub String);

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CompileError {}

fn tt_u64(bits: &[u64]) -> u64 {
    bits.first().copied().unwrap_or(0)
}

/// Builds a CSR adjacency list from (key, value) pairs.
fn csr(n: usize, pairs: &[(u32, u32)]) -> (Vec<u32>, Vec<u32>) {
    let mut off = vec![0u32; n + 1];
    for &(k, _) in pairs {
        off[k as usize + 1] += 1;
    }
    for i in 0..n {
        off[i + 1] += off[i];
    }
    let mut pos = off.clone();
    let mut vals = vec![0u32; pairs.len()];
    for &(k, v) in pairs {
        vals[pos[k as usize] as usize] = v;
        pos[k as usize] += 1;
    }
    (off, vals)
}

impl Compiled {
    pub fn new(netlist: Netlist, lib: &CellLibrary, clock: &str) -> Result<Compiled, CompileError> {
        let nnets = netlist.nets.len();
        let clock_net = netlist
            .port(clock)
            .and_then(|p| p.bits.first().copied())
            .ok_or_else(|| CompileError(format!("no clock port {clock}")))?;

        // Cell definitions in use.
        let mut defs: Vec<CellDef> = Vec::new();
        let mut def_index: HashMap<String, u16> = HashMap::new();
        let mut cell_def = Vec::with_capacity(netlist.cells.len());
        for c in &netlist.cells {
            let idx = match def_index.get(&c.ty) {
                Some(&i) => i,
                None => {
                    let d = lib.cells.get(&c.ty).ok_or_else(|| {
                        let why = lib.unsupported.get(&c.ty).map(String::as_str).unwrap_or("not in library");
                        CompileError(format!("cell {} has type {} which cannot be simulated: {why}", c.name, c.ty))
                    })?;
                    defs.push(d.clone());
                    let i = (defs.len() - 1) as u16;
                    def_index.insert(c.ty.clone(), i);
                    i
                }
            };
            cell_def.push(idx);
        }

        let mut driver = vec![Driver::None; nnets];
        driver[NET_CONST0 as usize] = Driver::Const(false);
        driver[NET_CONST1 as usize] = Driver::Const(true);

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for p in &netlist.ports {
            let b = PortBinding { name: p.name.clone(), bits: p.bits.clone() };
            match p.dir {
                PortDir::Input => {
                    for (bit, &n) in p.bits.iter().enumerate() {
                        driver[n as usize] = Driver::Input(inputs.len() as u16, bit as u8);
                    }
                    inputs.push(b);
                }
                PortDir::Output => outputs.push(b),
            }
        }

        let set_driver = |driver: &mut Vec<Driver>, net: NetId, d: Driver, who: &str| -> Result<(), CompileError> {
            match driver[net as usize] {
                Driver::None => {
                    driver[net as usize] = d;
                    Ok(())
                }
                prev => Err(CompileError(format!("net {net} driven by both {prev:?} and {who}"))),
            }
        };

        // Collect raw nodes and flops.
        struct RawNode {
            cell: u32,
            output: u8,
            out: NetId,
            ins: Vec<NetId>,
            tt: Vec<u64>,
        }
        let mut raw_nodes: Vec<RawNode> = Vec::new();
        let mut flop_cell = Vec::new();
        let mut flop_ins: Vec<Vec<NetId>> = Vec::new();
        let mut flop_tt = Vec::new();
        let mut flop_outs: Vec<Vec<(NetId, u8)>> = Vec::new();
        let mut flop_idempotent = Vec::new();

        for (ci, c) in netlist.cells.iter().enumerate() {
            let def = &defs[cell_def[ci] as usize];
            let pin_net = |name: &str| c.pins.iter().find(|p| p.name == name).map(|p| p.net);
            if let Some(seq) = &def.seq {
                if seq.clock_inverted {
                    return Err(CompileError(format!("{}: falling-edge flop", c.name)));
                }
                if pin_net(&seq.clock_pin) != Some(clock_net) {
                    return Err(CompileError(format!("{}: not clocked by {clock}", c.name)));
                }
                if seq.data_inputs.len() > 5 {
                    return Err(CompileError(format!("{}: flop with {} inputs", c.name, seq.data_inputs.len())));
                }
                let fi = flop_cell.len() as u32;
                let ins: Vec<NetId> = seq
                    .data_inputs
                    .iter()
                    .map(|p| pin_net(p).unwrap_or(NET_CONST0))
                    .collect();
                let mut outs = Vec::new();
                for (oi, o) in def.outputs.iter().enumerate() {
                    if let Some(net) = pin_net(&o.name) {
                        let f = (o.table.get(0) as u8) | ((o.table.get(1) as u8) << 1);
                        set_driver(&mut driver, net, Driver::Flop(fi, oi as u8), &c.name)?;
                        outs.push((net, f));
                    }
                }
                let tt = tt_u64(&seq.next_state.bits);
                // Idempotent: f(x, f(x, q)) == f(x, q) for all x, q.
                let nd = seq.data_inputs.len();
                let mut idem = true;
                for x in 0..(1u64 << nd) {
                    for q in 0..2u64 {
                        let n1 = (tt >> (x | (q << nd))) & 1;
                        let n2 = (tt >> (x | (n1 << nd))) & 1;
                        idem &= n1 == n2;
                    }
                }
                flop_cell.push(ci as u32);
                flop_ins.push(ins);
                flop_tt.push(tt);
                flop_outs.push(outs);
                flop_idempotent.push(idem);
            } else {
                let ins: Vec<NetId> = def.inputs.iter().map(|p| pin_net(p).unwrap_or(NET_CONST0)).collect();
                for (oi, o) in def.outputs.iter().enumerate() {
                    let Some(net) = pin_net(&o.name) else { continue };
                    let ni = raw_nodes.len() as u32;
                    set_driver(&mut driver, net, Driver::Node(ni), &c.name)?;
                    raw_nodes.push(RawNode { cell: ci as u32, output: oi as u8, out: net, ins: ins.clone(), tt: o.table.bits.clone() });
                }
            }
        }

        let mut is_read = vec![false; nnets];
        for c in &netlist.cells {
            for p in c.pins.iter().filter(|p| p.dir == PortDir::Input) {
                is_read[p.net as usize] = true;
            }
        }
        for o in &outputs {
            for &b in &o.bits {
                is_read[b as usize] = true;
            }
        }
        if let Some(i) = (0..nnets).find(|&i| driver[i] == Driver::None && is_read[i]) {
            return Err(CompileError(format!("net {} is read but has no driver", netlist.net_name(i as NetId))));
        }

        // Levelize nodes (Kahn's algorithm over node -> node edges).
        let n = raw_nodes.len();
        let mut level = vec![0u32; n];
        let mut indeg = vec![0u32; n];
        let mut succ_pairs = Vec::new();
        for (i, rn) in raw_nodes.iter().enumerate() {
            for &inp in &rn.ins {
                if let Driver::Node(src) = driver[inp as usize] {
                    succ_pairs.push((src, i as u32));
                    indeg[i] += 1;
                }
            }
        }
        let (succ_off, succ) = csr(n, &succ_pairs);
        let mut queue: Vec<u32> = (0..n as u32).filter(|&i| indeg[i as usize] == 0).collect();
        let mut head = 0;
        while head < queue.len() {
            let u = queue[head] as usize;
            head += 1;
            for &v in &succ[succ_off[u] as usize..succ_off[u + 1] as usize] {
                level[v as usize] = level[v as usize].max(level[u] + 1);
                indeg[v as usize] -= 1;
                if indeg[v as usize] == 0 {
                    queue.push(v);
                }
            }
        }
        if queue.len() != n {
            let stuck: Vec<String> = (0..n)
                .filter(|&i| indeg[i] > 0)
                .take(8)
                .map(|i| netlist.cells[raw_nodes[i].cell as usize].name.clone())
                .collect();
            return Err(CompileError(format!("combinational loop through cells {}", stuck.join(", "))));
        }

        // Order nodes by level (stable), then remap.
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.sort_by_key(|&i| (level[i as usize], i));
        let mut new_index = vec![0u32; n];
        for (new, &old) in order.iter().enumerate() {
            new_index[old as usize] = new as u32;
        }
        for d in &mut driver {
            if let Driver::Node(old) = d {
                *d = Driver::Node(new_index[*old as usize]);
            }
        }

        let mut node_cell = Vec::with_capacity(n);
        let mut node_output = Vec::with_capacity(n);
        let mut node_out = Vec::with_capacity(n);
        let mut node_level = Vec::with_capacity(n);
        let mut node_in_off = vec![0u32];
        let mut node_in = Vec::new();
        let mut node_tt = Vec::with_capacity(n);
        let mut node_big_tt = HashMap::new();
        for (new, &old) in order.iter().enumerate() {
            let rn = &raw_nodes[old as usize];
            node_cell.push(rn.cell);
            node_output.push(rn.output);
            node_out.push(rn.out);
            node_level.push(level[old as usize]);
            node_in.extend_from_slice(&rn.ins);
            node_in_off.push(node_in.len() as u32);
            if rn.ins.len() <= 6 {
                node_tt.push(tt_u64(&rn.tt));
            } else {
                node_tt.push(0);
                node_big_tt.insert(new as u32, rn.tt.clone());
            }
        }
        let max_level = node_level.last().copied().unwrap_or(0);

        let mut flop_in_off = vec![0u32];
        let mut flop_in = Vec::new();
        for ins in &flop_ins {
            flop_in.extend_from_slice(ins);
            flop_in_off.push(flop_in.len() as u32);
        }
        let mut flop_out_off = vec![0u32];
        let mut flop_out = Vec::new();
        for outs in &flop_outs {
            flop_out.extend_from_slice(outs);
            flop_out_off.push(flop_out.len() as u32);
        }

        // Fan-out.
        let mut fan_pairs = Vec::new();
        for i in 0..n {
            for &inp in &node_in[node_in_off[i] as usize..node_in_off[i + 1] as usize] {
                fan_pairs.push((inp, i as u32));
            }
        }
        fan_pairs.sort_unstable();
        fan_pairs.dedup();
        let (fan_off, fan_node) = csr(nnets, &fan_pairs);
        let mut ffan_pairs = Vec::new();
        for f in 0..flop_cell.len() {
            for &inp in &flop_in[flop_in_off[f] as usize..flop_in_off[f + 1] as usize] {
                ffan_pairs.push((inp, f as u32));
            }
        }
        ffan_pairs.sort_unstable();
        ffan_pairs.dedup();
        let (ffan_off, ffan_flop) = csr(nnets, &ffan_pairs);

        Ok(Compiled {
            netlist,
            defs,
            cell_def,
            node_cell,
            node_output,
            node_out,
            node_level,
            node_in_off,
            node_in,
            node_tt,
            node_big_tt,
            max_level,
            flop_cell,
            flop_in_off,
            flop_in,
            flop_tt,
            flop_out_off,
            flop_out,
            flop_idempotent,
            fan_off,
            fan_node,
            ffan_off,
            ffan_flop,
            driver,
            inputs,
            outputs,
            clock: clock_net,
        })
    }

    pub fn nodes(&self) -> usize {
        self.node_out.len()
    }

    pub fn flops(&self) -> usize {
        self.flop_cell.len()
    }

    pub fn node_inputs(&self, i: usize) -> &[NetId] {
        &self.node_in[self.node_in_off[i] as usize..self.node_in_off[i + 1] as usize]
    }

    pub fn flop_inputs(&self, f: usize) -> &[NetId] {
        &self.flop_in[self.flop_in_off[f] as usize..self.flop_in_off[f + 1] as usize]
    }

    pub fn flop_outputs(&self, f: usize) -> &[(NetId, u8)] {
        &self.flop_out[self.flop_out_off[f] as usize..self.flop_out_off[f + 1] as usize]
    }

    pub fn fanout_nodes(&self, net: NetId) -> &[u32] {
        &self.fan_node[self.fan_off[net as usize] as usize..self.fan_off[net as usize + 1] as usize]
    }

    pub fn fanout_flops(&self, net: NetId) -> &[u32] {
        &self.ffan_flop[self.ffan_off[net as usize] as usize..self.ffan_off[net as usize + 1] as usize]
    }

    /// Index of the flop implemented by netlist cell `cell`, if any.
    pub fn flop_of_cell(&self, cell: u32) -> Option<u32> {
        self.flop_cell.iter().position(|&c| c == cell).map(|i| i as u32)
    }

    pub fn input_port(&self, name: &str) -> Option<usize> {
        self.inputs.iter().position(|p| p.name == name)
    }

    pub fn output_port(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|p| p.name == name)
    }
}
