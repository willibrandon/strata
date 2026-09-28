//! Flattened gate-level netlists read from Yosys JSON.
//!
//! The Yosys netlist is hierarchical: one module per RTL instance, with leaf
//! cells from the cell library. Flattening assigns every bit a global net,
//! merges nets connected through ports, and records for each cell the module
//! instance that contains it, so gate-level objects can always be traced back
//! to the RTL hierarchy.

use std::collections::HashMap;

use serde_json::Value;

pub type NetId = u32;

/// Net 0 is constant 0 and net 1 is constant 1.
pub const NET_CONST0: NetId = 0;
pub const NET_CONST1: NetId = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDir {
    Input,
    Output,
}

#[derive(Debug, Clone)]
pub struct Net {
    /// Hierarchical names, e.g. `u_core.x_pc[3]`. Empty for anonymous nets.
    pub names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CellPin {
    pub name: String,
    pub dir: PortDir,
    pub net: NetId,
}

#[derive(Debug, Clone)]
pub struct Cell {
    /// Full hierarchical instance name, e.g. `u_core.u_alu.$abc$123$auto_45`.
    pub name: String,
    /// Library cell type, e.g. `sky130_fd_sc_hd__nand2_1`.
    pub ty: String,
    /// Index into `Netlist::instances` of the containing module instance.
    pub instance: u32,
    pub pins: Vec<CellPin>,
    /// RTL source location recorded by synthesis, when available.
    pub src: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Port {
    pub name: String,
    pub dir: PortDir,
    /// Bit 0 first.
    pub bits: Vec<NetId>,
}

/// A module instance in the RTL hierarchy.
#[derive(Debug, Clone)]
pub struct Instance {
    /// Dotted instance path; empty for the top module.
    pub path: String,
    /// RTL module name (e.g. `st_alu`).
    pub module: String,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    pub cells: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Netlist {
    pub top: String,
    pub nets: Vec<Net>,
    pub cells: Vec<Cell>,
    pub ports: Vec<Port>,
    pub instances: Vec<Instance>,
    /// Number of `x`/`z` constant bits that were tied to 0.
    pub undef_bits: usize,
}

#[derive(Debug)]
pub struct NetlistError(pub String);

impl std::fmt::Display for NetlistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NetlistError {}

fn err<T>(msg: impl Into<String>) -> Result<T, NetlistError> {
    Err(NetlistError(msg.into()))
}

struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn make(&mut self) -> u32 {
        let id = self.parent.len() as u32;
        self.parent.push(id);
        id
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let p = self.parent[x as usize];
            self.parent[x as usize] = self.parent[p as usize];
            x = p;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            // Keep constants as representatives.
            let (keep, drop) = if a < b { (a, b) } else { (b, a) };
            self.parent[drop as usize] = keep;
        }
    }
}

struct Builder<'a> {
    modules: &'a serde_json::Map<String, Value>,
    uf: UnionFind,
    names: Vec<(u32, String)>,
    cells: Vec<Cell>,
    instances: Vec<Instance>,
    undef_bits: usize,
}

/// Strips the per-instance suffix yosys-slang adds (`st_alu$strata_soc.u_core.u_alu`).
fn rtl_module_name(name: &str) -> &str {
    name.split('$').next().unwrap_or(name)
}

impl<'a> Builder<'a> {
    fn is_leaf(&self, ty: &str) -> bool {
        match self.modules.get(ty) {
            None => true,
            Some(m) => m
                .get("attributes")
                .and_then(|a| a.get("blackbox"))
                .is_some_and(|v| v != &Value::Null && v != "0" && v != &Value::from(0)),
        }
    }

    /// Resolves a JSON bit (number or constant string) through `local`.
    fn bit(&mut self, v: &Value, local: &mut HashMap<u64, u32>) -> Result<u32, NetlistError> {
        match v {
            Value::Number(n) => {
                let n = n.as_u64().ok_or_else(|| NetlistError("bad bit".into()))?;
                if let Some(&g) = local.get(&n) {
                    Ok(g)
                } else {
                    let g = self.uf.make();
                    local.insert(n, g);
                    Ok(g)
                }
            }
            Value::String(s) => match s.as_str() {
                "0" => Ok(NET_CONST0),
                "1" => Ok(NET_CONST1),
                "x" | "z" => {
                    self.undef_bits += 1;
                    Ok(NET_CONST0)
                }
                _ => err(format!("bad constant bit {s:?}")),
            },
            _ => err("bad bit value"),
        }
    }

    fn instantiate(
        &mut self,
        mod_name: &str,
        path: &str,
        parent: Option<u32>,
        mut local: HashMap<u64, u32>,
    ) -> Result<u32, NetlistError> {
        let module = self
            .modules
            .get(mod_name)
            .ok_or_else(|| NetlistError(format!("module {mod_name} not found")))?;
        let inst_id = self.instances.len() as u32;
        self.instances.push(Instance {
            path: path.to_string(),
            module: rtl_module_name(mod_name).to_string(),
            parent,
            children: Vec::new(),
            cells: Vec::new(),
        });
        if let Some(p) = parent {
            self.instances[p as usize].children.push(inst_id);
        }
        let prefix = if path.is_empty() { String::new() } else { format!("{path}.") };

        // Net names.
        if let Some(nn) = module.get("netnames").and_then(Value::as_object) {
            for (name, info) in nn {
                if info.get("hide_name").and_then(Value::as_u64) == Some(1) {
                    continue;
                }
                let bits = info.get("bits").and_then(Value::as_array).cloned().unwrap_or_default();
                let attrs = info.get("attributes");
                let offset = info.get("offset").and_then(Value::as_i64).unwrap_or(0);
                let upto = info.get("upto").and_then(Value::as_u64) == Some(1);
                let _ = attrs;
                let width = bits.len() as i64;
                for (j, b) in bits.iter().enumerate() {
                    let g = self.bit(b, &mut local)?;
                    if g == NET_CONST0 || g == NET_CONST1 {
                        continue;
                    }
                    let idx = if upto { offset + width - 1 - j as i64 } else { offset + j as i64 };
                    let full = if width == 1 && offset == 0 {
                        format!("{prefix}{name}")
                    } else {
                        format!("{prefix}{name}[{idx}]")
                    };
                    self.names.push((g, full));
                }
            }
        }

        // Cells.
        if let Some(cells) = module.get("cells").and_then(Value::as_object) {
            for (cname, c) in cells {
                let ty = c.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
                let conns = c
                    .get("connections")
                    .and_then(Value::as_object)
                    .ok_or_else(|| NetlistError(format!("cell {cname} has no connections")))?;
                if self.is_leaf(&ty) {
                    let dirs = c.get("port_directions").and_then(Value::as_object);
                    let mut pins = Vec::new();
                    for (pin, bits) in conns {
                        let bits = bits.as_array().cloned().unwrap_or_default();
                        let dir = match dirs.and_then(|d| d.get(pin)).and_then(Value::as_str) {
                            Some("output") => PortDir::Output,
                            Some("input") => PortDir::Input,
                            other => {
                                return err(format!("cell {prefix}{cname}: pin {pin} direction {other:?}"))
                            }
                        };
                        if bits.len() != 1 {
                            return err(format!("cell {prefix}{cname}: pin {pin} is {} bits", bits.len()));
                        }
                        let net = self.bit(&bits[0], &mut local)?;
                        pins.push(CellPin { name: pin.clone(), dir, net });
                    }
                    let src = c
                        .get("attributes")
                        .and_then(|a| a.get("src"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let id = self.cells.len() as u32;
                    self.cells.push(Cell { name: format!("{prefix}{cname}"), ty, instance: inst_id, pins, src });
                    self.instances[inst_id as usize].cells.push(id);
                } else {
                    // Submodule: map its port bits to our nets.
                    let sub = &self.modules[&ty];
                    let ports = sub.get("ports").and_then(Value::as_object).cloned().unwrap_or_default();
                    let mut sub_local: HashMap<u64, u32> = HashMap::new();
                    for (pname, pinfo) in &ports {
                        let sub_bits = pinfo.get("bits").and_then(Value::as_array).cloned().unwrap_or_default();
                        let Some(par_bits) = conns.get(pname).and_then(Value::as_array).cloned() else {
                            continue; // unconnected port
                        };
                        for (sb, pb) in sub_bits.iter().zip(par_bits.iter()) {
                            let g = self.bit(pb, &mut local)?;
                            match sb {
                                Value::Number(n) => {
                                    let n = n.as_u64().unwrap();
                                    if let Some(&prev) = sub_local.get(&n) {
                                        self.uf.union(prev, g);
                                    } else {
                                        sub_local.insert(n, g);
                                    }
                                }
                                Value::String(_) => {
                                    // Port bit tied to a constant inside the submodule.
                                    let c = self.bit(sb, &mut HashMap::new())?;
                                    self.uf.union(c, g);
                                }
                                _ => return err("bad port bit"),
                            }
                        }
                    }
                    let sub_path = format!("{prefix}{cname}");
                    self.instantiate(&ty, &sub_path, Some(inst_id), sub_local)?;
                }
            }
        }
        Ok(inst_id)
    }
}

impl Netlist {
    /// Reads a Yosys JSON netlist and flattens module `top`.
    pub fn from_yosys_json(src: &str, top: &str) -> Result<Netlist, NetlistError> {
        let root: Value = serde_json::from_str(src).map_err(|e| NetlistError(format!("JSON: {e}")))?;
        let modules = root
            .get("modules")
            .and_then(Value::as_object)
            .ok_or_else(|| NetlistError("no modules".into()))?;
        let top_mod = modules.get(top).ok_or_else(|| NetlistError(format!("top module {top} not found")))?;

        let mut b = Builder {
            modules,
            uf: UnionFind { parent: vec![NET_CONST0, NET_CONST1] },
            names: Vec::new(),
            cells: Vec::new(),
            instances: Vec::new(),
            undef_bits: 0,
        };

        // Top-level ports get nets first so they are easy to find.
        let mut local = HashMap::new();
        let mut ports = Vec::new();
        if let Some(ps) = top_mod.get("ports").and_then(Value::as_object) {
            for (name, p) in ps {
                let dir = match p.get("direction").and_then(Value::as_str) {
                    Some("input") => PortDir::Input,
                    Some("output") => PortDir::Output,
                    other => return err(format!("port {name}: unsupported direction {other:?}")),
                };
                let mut bits = Vec::new();
                for bv in p.get("bits").and_then(Value::as_array).cloned().unwrap_or_default() {
                    bits.push(b.bit(&bv, &mut local)?);
                }
                ports.push(Port { name: name.clone(), dir, bits });
            }
        }
        b.instantiate(top, "", None, local)?;

        // Canonicalize and compact net ids.
        let n = b.uf.parent.len();
        let mut remap = vec![u32::MAX; n];
        let mut next = 0u32;
        for i in 0..n as u32 {
            let r = b.uf.find(i);
            if remap[r as usize] == u32::MAX {
                remap[r as usize] = next;
                next += 1;
            }
            remap[i as usize] = remap[r as usize];
        }
        debug_assert_eq!(remap[0], NET_CONST0);
        debug_assert_eq!(remap[1], NET_CONST1);
        let mut nets = vec![Net { names: Vec::new() }; next as usize];
        nets[0].names.push("1'b0".into());
        nets[1].names.push("1'b1".into());
        for (g, name) in b.names {
            nets[remap[g as usize] as usize].names.push(name);
        }
        for net in &mut nets {
            net.names.sort_by_key(|s| (s.matches('.').count(), s.len()));
            net.names.dedup();
        }
        for c in &mut b.cells {
            for p in &mut c.pins {
                p.net = remap[p.net as usize];
            }
        }
        for p in &mut ports {
            for bit in &mut p.bits {
                *bit = remap[*bit as usize];
            }
        }
        Ok(Netlist {
            top: top.to_string(),
            nets,
            cells: b.cells,
            ports,
            instances: b.instances,
            undef_bits: b.undef_bits,
        })
    }

    pub fn load(path: &std::path::Path, top: &str) -> Result<Netlist, NetlistError> {
        let src = std::fs::read_to_string(path)
            .map_err(|e| NetlistError(format!("{}: {e}", path.display())))?;
        Self::from_yosys_json(&src, top)
    }

    pub fn port(&self, name: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.name == name)
    }

    /// Finds a net by any of its hierarchical names.
    pub fn net_by_name(&self, name: &str) -> Option<NetId> {
        self.nets.iter().position(|n| n.names.iter().any(|s| s == name)).map(|i| i as NetId)
    }

    /// A map from every net name to its net, for repeated lookups.
    pub fn name_index(&self) -> HashMap<&str, NetId> {
        let mut m = HashMap::new();
        for (i, n) in self.nets.iter().enumerate() {
            for s in &n.names {
                m.insert(s.as_str(), i as NetId);
            }
        }
        m
    }

    /// The best display name of a net (shortest, least nested).
    pub fn net_name(&self, id: NetId) -> String {
        self.nets[id as usize].names.first().cloned().unwrap_or_else(|| format!("n{id}"))
    }

    pub fn instance_by_path(&self, path: &str) -> Option<u32> {
        self.instances.iter().position(|i| i.path == path).map(|i| i as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{
      "modules": {
        "inv": { "attributes": { "blackbox": "00000000000000000000000000000001" },
                 "ports": { "A": {"direction":"input","bits":[2]}, "Y": {"direction":"output","bits":[3]} },
                 "cells": {}, "netnames": {} },
        "leaf$top.u": {
          "ports": { "a": {"direction":"input","bits":[2]}, "y": {"direction":"output","bits":[3]},
                     "same": {"direction":"output","bits":[3]} },
          "cells": { "$abc$1": { "type": "inv", "port_directions": {"A":"input","Y":"output"},
                                 "connections": {"A":[2],"Y":[3]}, "attributes": {"src":"x.sv:1"} } },
          "netnames": { "a": {"hide_name":0,"bits":[2]}, "y": {"hide_name":0,"bits":[3]} } },
        "top": {
          "ports": { "i": {"direction":"input","bits":[2]}, "o": {"direction":"output","bits":[3,4]} },
          "cells": { "u": { "type": "leaf$top.u", "connections": {"a":[2],"y":[3],"same":[4]} },
                     "t": { "type": "inv", "port_directions": {"A":"input","Y":"output"},
                            "connections": {"A":["1"],"Y":[5]} } },
          "netnames": { "i": {"hide_name":0,"bits":[2]}, "o": {"hide_name":0,"bits":[3,4]},
                        "k": {"hide_name":0,"bits":[5]} } }
      } }"#;

    #[test]
    fn flattens_and_merges_port_aliases() {
        let n = Netlist::from_yosys_json(JSON, "top").unwrap();
        assert_eq!(n.cells.len(), 2);
        let o = n.port("o").unwrap();
        // "y" and "same" are the same net inside u, so o[0] and o[1] merge.
        assert_eq!(o.bits[0], o.bits[1]);
        let inner = n.cells.iter().find(|c| c.name == "u.$abc$1").unwrap();
        assert_eq!(n.instances[inner.instance as usize].path, "u");
        assert_eq!(n.instances[inner.instance as usize].module, "leaf");
        assert_eq!(inner.src.as_deref(), Some("x.sv:1"));
        let t = n.cells.iter().find(|c| c.name == "t").unwrap();
        assert_eq!(t.pins.iter().find(|p| p.name == "A").unwrap().net, NET_CONST1);
        assert_eq!(n.net_by_name("u.a"), n.net_by_name("i"));
        assert_eq!(n.net_by_name("o[1]"), n.net_by_name("u.y"));
    }
}
