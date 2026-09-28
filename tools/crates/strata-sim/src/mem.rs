//! Behavioral memory macros: the only memory abstraction in the machine.
//!
//! Semantics are those of `rtl/mem/st_sram_dp.sv`: on each rising edge, for
//! each enabled port, a write (any `we` bit set) updates the enabled bytes and
//! leaves `rdata` unchanged, and a read loads `rdata` from the contents before
//! any write at that edge. Disabled ports hold `rdata`. When both ports write
//! the same byte in one cycle, port B's data is stored.

use serde::{Deserialize, Serialize};

use crate::ports::MemPort;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sram {
    pub name: String,
    pub words: Vec<u32>,
    pub a_rdata: u32,
    pub b_rdata: u32,
    /// Ports that may never write (the ROM's, and read-only scan-out ports).
    pub a_writable: bool,
    pub b_writable: bool,
}

/// One access performed at a clock edge (for the debugger's transaction log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemAccess {
    pub port_b: bool,
    pub word: u32,
    pub we: u8,
    pub wdata: u32,
    /// For reads, the value returned; for writes, the old contents.
    pub data: u32,
}

impl Sram {
    pub fn new(name: &str, words: usize, a_writable: bool, b_writable: bool) -> Sram {
        Sram { name: name.to_string(), words: vec![0; words], a_rdata: 0, b_rdata: 0, a_writable, b_writable }
    }

    /// Loads little-endian bytes starting at word 0.
    pub fn load_bytes(&mut self, bytes: &[u8]) {
        for (i, chunk) in bytes.chunks(4).enumerate() {
            if i >= self.words.len() {
                break;
            }
            let mut w = [0u8; 4];
            w[..chunk.len()].copy_from_slice(chunk);
            self.words[i] = u32::from_le_bytes(w);
        }
    }

    /// Clock edge. Returns the accesses performed (at most one per port).
    pub fn clock(&mut self, a: &MemPort, b: &MemPort, log: Option<&mut Vec<MemAccess>>) {
        let n = self.words.len() as u32;
        let (ai, bi) = (a.addr % n, b.addr % n);
        let (old_a, old_b) = (self.words[ai as usize], self.words[bi as usize]);
        let a_we = if self.a_writable { a.we & 0xF } else { 0 };
        let b_we = if self.b_writable { b.we & 0xF } else { 0 };
        if a.en && a_we == 0 {
            self.a_rdata = old_a;
        }
        if b.en && b_we == 0 {
            self.b_rdata = old_b;
        }
        if a.en && a_we != 0 {
            self.words[ai as usize] = merge(self.words[ai as usize], a.wdata, a_we);
        }
        if b.en && b_we != 0 {
            self.words[bi as usize] = merge(self.words[bi as usize], b.wdata, b_we);
        }
        if let Some(log) = log {
            if a.en {
                log.push(MemAccess { port_b: false, word: ai, we: a_we, wdata: a.wdata, data: old_a });
            }
            if b.en {
                log.push(MemAccess { port_b: true, word: bi, we: b_we, wdata: b.wdata, data: old_b });
            }
        }
    }
}

fn merge(old: u32, new: u32, we: u8) -> u32 {
    let mut m = 0u32;
    for i in 0..4 {
        if we & (1 << i) != 0 {
            m |= 0xFF << (8 * i);
        }
    }
    (old & !m) | (new & m)
}

/// The machine's four memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memories {
    pub rom: Sram,
    pub ram: Sram,
    pub vram: Sram,
    pub font: Sram,
}

impl Default for Memories {
    fn default() -> Self {
        Memories {
            rom: Sram::new("rom", 1024, false, false),
            ram: Sram::new("ram", 16384, false, true),
            vram: Sram::new("vram", 2048, true, false),
            font: Sram::new("font", 1024, true, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_before_write_and_byte_enables() {
        let mut m = Sram::new("t", 4, true, true);
        m.words[1] = 0x11223344;
        let rd = MemPort { en: true, we: 0, addr: 1, wdata: 0 };
        let wr = MemPort { en: true, we: 0b0010, addr: 1, wdata: 0xAABBCCDD };
        m.clock(&rd, &wr, None);
        assert_eq!(m.a_rdata, 0x11223344); // old contents
        assert_eq!(m.words[1], 0x1122CC44);
        let idle = MemPort::default();
        m.clock(&idle, &idle, None);
        assert_eq!(m.a_rdata, 0x11223344); // held
        let wa = MemPort { en: true, we: 0xF, addr: 2, wdata: 1 };
        let wb = MemPort { en: true, we: 0xF, addr: 2, wdata: 2 };
        m.clock(&wa, &wb, None);
        assert_eq!(m.words[2], 2); // port B wins
    }
}
