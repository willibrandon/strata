//! Named signals of the design, resolved to netlist nets.
//!
//! The debugger reads architectural and microarchitectural state directly
//! from the nets of the gate-level netlist: registers from the register
//! file's flip-flops, pipeline state from the stage registers, CSRs from the
//! CSR unit. Names are the RTL names that synthesis preserves.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use strata_netlist::{NetId, Netlist};

/// A multi-bit signal: its name and its nets, least significant first.
/// `lsb` is the RTL index of the first bit (e.g. 2 for `logic [31:2] mepc_q`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    pub name: String,
    pub bits: Vec<NetId>,
    #[serde(default)]
    pub lsb: u32,
}

impl Signal {
    pub fn width(&self) -> usize {
        self.bits.len()
    }
}

/// Resolves `name` (e.g. `u_core.x_pc`) to a signal: a single net called
/// exactly `name`, or nets `name[i]`, `name[i+1]`, ... from the lowest
/// index present up to the first gap.
pub fn resolve(index: &HashMap<&str, NetId>, name: &str) -> Option<Signal> {
    if let Some(&n) = index.get(name) {
        return Some(Signal { name: name.to_string(), bits: vec![n], lsb: 0 });
    }
    let lsb = (0..64u32).find(|i| index.contains_key(format!("{name}[{i}]").as_str()))?;
    let mut bits = Vec::new();
    while let Some(&n) = index.get(format!("{name}[{}]", lsb as usize + bits.len()).as_str()) {
        bits.push(n);
    }
    Some(Signal { name: name.to_string(), bits, lsb })
}

/// One pipeline stage's observable state.
#[derive(Debug, Clone)]
pub struct StageProbes {
    pub name: &'static str,
    pub valid: Signal,
    pub pc: Signal,
    pub insn: Option<Signal>,
}

/// The signals the debugger's architectural and pipeline views use.
#[derive(Debug, Clone)]
pub struct ArchProbes {
    /// x1..x31 (index 0 unused).
    pub regs: Vec<Option<Signal>>,
    pub stages: Vec<StageProbes>,
    pub csr: Vec<(&'static str, Signal)>,
    pub mode: Signal,
    pub ibus: BusProbes,
    pub dbus: BusProbes,
    pub stall: Vec<(&'static str, Signal)>,
    /// Names that could not be resolved (reported, not fatal).
    pub missing: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BusProbes {
    pub cmd_valid: Signal,
    pub cmd_ready: Signal,
    pub cmd_addr: Signal,
    pub cmd_we: Option<Signal>,
    pub cmd_be: Option<Signal>,
    pub cmd_wdata: Option<Signal>,
    pub rsp_valid: Signal,
    pub rsp_rdata: Signal,
    pub rsp_err: Signal,
}

fn constant(name: &str, value: bool) -> Signal {
    Signal {
        name: name.to_string(),
        bits: vec![if value { strata_netlist::NET_CONST1 } else { strata_netlist::NET_CONST0 }],
        lsb: 0,
    }
}

impl ArchProbes {
    pub fn resolve(netlist: &Netlist) -> ArchProbes {
        let index = netlist.name_index();
        let mut missing = Vec::new();
        let mut get = |name: &str| -> Option<Signal> {
            let s = resolve(&index, name);
            if s.is_none() {
                missing.push(name.to_string());
            }
            s
        };
        let mut regs = vec![None];
        for i in 1..32 {
            regs.push(get(&format!("u_core.u_rf.gen_x[{i}].q")));
        }
        let stage = |name: &'static str, valid: &str, pc: &str, insn: Option<&str>, get: &mut dyn FnMut(&str) -> Option<Signal>| {
            let v = get(valid).unwrap_or_else(|| constant(valid, false));
            let p = get(pc).unwrap_or_else(|| constant(pc, false));
            StageProbes { name, valid: v, pc: p, insn: insn.and_then(|n| get(n)) }
        };
        let stages = vec![
            stage("F", "u_core.u_fetch.pend_q", "u_core.u_fetch.pend_pc_q", None, &mut get),
            stage("D", "u_core.f_valid", "u_core.f_pc", Some("u_core.f_insn"), &mut get),
            stage("X", "u_core.x_valid", "u_core.x_pc", Some("u_core.x_insn"), &mut get),
            stage("M", "u_core.m_valid", "u_core.m_pc", Some("u_core.m_insn"), &mut get),
            stage("W", "u_core.w_valid", "u_core.w_pc", Some("u_core.w_insn"), &mut get),
        ];
        let mut csr = Vec::new();
        for (label, name) in [
            ("mstatus.MIE", "u_core.u_csr.mie_q"),
            ("mstatus.MPIE", "u_core.u_csr.mpie_q"),
            ("mstatus.MPP=M", "u_core.u_csr.mpp_m_q"),
            ("mstatus.MPRV", "u_core.u_csr.mprv_q"),
            ("mie.MSIE", "u_core.u_csr.msie_q"),
            ("mie.MTIE", "u_core.u_csr.mtie_q"),
            ("mie.MEIE", "u_core.u_csr.meie_q"),
            ("mtvec.BASE", "u_core.u_csr.mtvec_base_q"),
            ("mtvec.MODE", "u_core.u_csr.mtvec_vec_q"),
            ("mscratch", "u_core.u_csr.mscratch_q"),
            ("mepc", "u_core.u_csr.mepc_q"),
            ("mcause.INT", "u_core.u_csr.mcause_int_q"),
            ("mcause.CODE", "u_core.u_csr.mcause_code_q"),
            ("mtval", "u_core.u_csr.mtval_q"),
            ("mcounteren", "u_core.u_csr.mcounteren_q"),
            ("mcycle", "u_core.u_csr.mcycle_q"),
            ("minstret", "u_core.u_csr.minstret_q"),
            ("pmpcfg0", "u_core.pmp_cfg"),
        ] {
            if let Some(s) = get(name) {
                csr.push((label, s));
            }
        }
        let mode = get("u_core.u_csr.mode_q").unwrap_or_else(|| constant("mode", true));
        let bus = |p: &str, get: &mut dyn FnMut(&str) -> Option<Signal>, data: bool| BusProbes {
            cmd_valid: get(&format!("u_core.{p}_cmd_valid")).unwrap_or_else(|| constant("cmd_valid", false)),
            cmd_ready: get(&format!("u_core.{p}_cmd_ready")).unwrap_or_else(|| constant("cmd_ready", true)),
            cmd_addr: get(&format!("u_core.{p}_cmd_addr")).unwrap_or_else(|| constant("cmd_addr", false)),
            cmd_we: if data { get(&format!("u_core.{p}_cmd_we")) } else { None },
            cmd_be: if data { get(&format!("u_core.{p}_cmd_be")) } else { None },
            cmd_wdata: if data { get(&format!("u_core.{p}_cmd_wdata")) } else { None },
            rsp_valid: get(&format!("u_core.{p}_rsp_valid")).unwrap_or_else(|| constant("rsp_valid", false)),
            rsp_rdata: get(&format!("u_core.{p}_rsp_rdata")).unwrap_or_else(|| constant("rsp_rdata", false)),
            rsp_err: get(&format!("u_core.{p}_rsp_err")).unwrap_or_else(|| constant("rsp_err", false)),
        };
        let ibus = bus("ibus", &mut get, false);
        let dbus = bus("dbus", &mut get, true);
        let mut stall = Vec::new();
        for (label, name) in [
            ("load-use hazard (X)", "u_core.x_hazard"),
            ("W busy", "u_core.w_busy"),
            ("W flush", "u_core.w_flush"),
            ("X redirect", "u_core.x_redirect"),
            ("interrupt taken (M)", "u_core.take_irq"),
            ("trap (W)", "u_core.trap_take"),
        ] {
            if let Some(s) = get(name) {
                stall.push((label, s));
            }
        }
        ArchProbes { regs, stages, csr, mode, ibus, dbus, stall, missing }
    }
}
