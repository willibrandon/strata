//! The ports of `strata_soc`, the boundary of the gate-level model
//! (docs/abstractions.md). Every logic engine exchanges exactly these values
//! with the host once per clock cycle.

use serde::{Deserialize, Serialize};

/// Inputs of `strata_soc` (other than `clk`, which the engine owns).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocIn {
    pub rst_in: bool,
    pub uart_rx: bool,
    pub btn: u8,
    pub spi_miso: bool,
    pub rom_a_rdata: u32,
    pub rom_b_rdata: u32,
    pub ram_a_rdata: u32,
    pub ram_b_rdata: u32,
    pub vram_a_rdata: u32,
    pub vram_b_rdata: u32,
    pub font_a_rdata: u32,
    pub font_b_rdata: u32,
}

/// One memory-macro port as driven by the logic in a cycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemPort {
    pub en: bool,
    pub we: u8,
    pub addr: u32,
    pub wdata: u32,
}

/// The retirement-trace outputs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracePort {
    pub valid: bool,
    pub trap: bool,
    pub intr: bool,
    pub cause: u8,
    pub pc: u32,
    pub insn: u32,
    pub next_pc: u32,
    pub mode: u8,
    pub rd_we: bool,
    pub rd: u8,
    pub rd_wdata: u32,
}

/// Outputs of `strata_soc`, valid after combinational evaluation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocOut {
    pub uart_tx: bool,
    pub led: u8,
    pub spi_cs_n: bool,
    pub spi_sck: bool,
    pub spi_mosi: bool,
    pub vid_rgb: u16, // 0xRGB, 4 bits per channel
    pub vid_hs: bool,
    pub vid_vs: bool,
    pub vid_de: bool,
    pub halted: bool,
    pub halt_code: u8,
    pub rom_a: MemPort,
    pub rom_b: MemPort,
    pub ram_a: MemPort,
    pub ram_b: MemPort,
    pub vram_a: MemPort,
    pub vram_b: MemPort,
    pub font_a: MemPort,
    pub font_b: MemPort,
    pub trace: TracePort,
}

/// Port names and widths as they appear in the netlist, for engines that bind
/// by name. `(name, width)`.
pub const SOC_INPUTS: &[(&str, u32)] = &[
    ("rst_in", 1), ("uart_rx", 1), ("btn", 7), ("spi_miso", 1),
    ("rom_a_rdata", 32), ("rom_b_rdata", 32), ("ram_a_rdata", 32), ("ram_b_rdata", 32),
    ("vram_a_rdata", 32), ("vram_b_rdata", 32), ("font_a_rdata", 32), ("font_b_rdata", 32),
];

pub const SOC_OUTPUTS: &[(&str, u32)] = &[
    ("uart_tx", 1), ("led", 8), ("spi_cs_n", 1), ("spi_sck", 1), ("spi_mosi", 1),
    ("vid_r", 4), ("vid_g", 4), ("vid_b", 4), ("vid_hs", 1), ("vid_vs", 1), ("vid_de", 1),
    ("halted", 1), ("halt_code", 8),
    ("rom_a_en", 1), ("rom_a_addr", 10), ("rom_b_en", 1), ("rom_b_addr", 10),
    ("ram_a_en", 1), ("ram_a_addr", 14),
    ("ram_b_en", 1), ("ram_b_we", 4), ("ram_b_addr", 14), ("ram_b_wdata", 32),
    ("vram_a_en", 1), ("vram_a_we", 4), ("vram_a_addr", 11), ("vram_a_wdata", 32),
    ("vram_b_en", 1), ("vram_b_addr", 11),
    ("font_a_en", 1), ("font_a_we", 4), ("font_a_addr", 10), ("font_a_wdata", 32),
    ("font_b_en", 1), ("font_b_addr", 10),
    ("trace_valid", 1), ("trace_trap", 1), ("trace_intr", 1), ("trace_cause", 5),
    ("trace_pc", 32), ("trace_insn", 32), ("trace_next_pc", 32), ("trace_mode", 2),
    ("trace_rd_we", 1), ("trace_rd", 5), ("trace_rd_wdata", 32),
];

impl SocIn {
    /// Value of an input port by netlist name.
    pub fn get(&self, name: &str) -> u64 {
        match name {
            "rst_in" => self.rst_in as u64,
            "uart_rx" => self.uart_rx as u64,
            "btn" => self.btn as u64,
            "spi_miso" => self.spi_miso as u64,
            "rom_a_rdata" => self.rom_a_rdata as u64,
            "rom_b_rdata" => self.rom_b_rdata as u64,
            "ram_a_rdata" => self.ram_a_rdata as u64,
            "ram_b_rdata" => self.ram_b_rdata as u64,
            "vram_a_rdata" => self.vram_a_rdata as u64,
            "vram_b_rdata" => self.vram_b_rdata as u64,
            "font_a_rdata" => self.font_a_rdata as u64,
            "font_b_rdata" => self.font_b_rdata as u64,
            _ => panic!("unknown input port {name}"),
        }
    }
}

impl SocOut {
    /// Sets an output port by netlist name (engines that bind by name).
    pub fn set(&mut self, name: &str, v: u64) {
        let b = v != 0;
        let v32 = v as u32;
        match name {
            "uart_tx" => self.uart_tx = b,
            "led" => self.led = v as u8,
            "spi_cs_n" => self.spi_cs_n = b,
            "spi_sck" => self.spi_sck = b,
            "spi_mosi" => self.spi_mosi = b,
            "vid_r" => self.vid_rgb = (self.vid_rgb & 0x0FF) | ((v as u16 & 0xF) << 8),
            "vid_g" => self.vid_rgb = (self.vid_rgb & 0xF0F) | ((v as u16 & 0xF) << 4),
            "vid_b" => self.vid_rgb = (self.vid_rgb & 0xFF0) | (v as u16 & 0xF),
            "vid_hs" => self.vid_hs = b,
            "vid_vs" => self.vid_vs = b,
            "vid_de" => self.vid_de = b,
            "halted" => self.halted = b,
            "halt_code" => self.halt_code = v as u8,
            "rom_a_en" => self.rom_a.en = b,
            "rom_a_addr" => self.rom_a.addr = v32,
            "rom_b_en" => self.rom_b.en = b,
            "rom_b_addr" => self.rom_b.addr = v32,
            "ram_a_en" => self.ram_a.en = b,
            "ram_a_addr" => self.ram_a.addr = v32,
            "ram_b_en" => self.ram_b.en = b,
            "ram_b_we" => self.ram_b.we = v as u8,
            "ram_b_addr" => self.ram_b.addr = v32,
            "ram_b_wdata" => self.ram_b.wdata = v32,
            "vram_a_en" => self.vram_a.en = b,
            "vram_a_we" => self.vram_a.we = v as u8,
            "vram_a_addr" => self.vram_a.addr = v32,
            "vram_a_wdata" => self.vram_a.wdata = v32,
            "vram_b_en" => self.vram_b.en = b,
            "vram_b_addr" => self.vram_b.addr = v32,
            "font_a_en" => self.font_a.en = b,
            "font_a_we" => self.font_a.we = v as u8,
            "font_a_addr" => self.font_a.addr = v32,
            "font_a_wdata" => self.font_a.wdata = v32,
            "font_b_en" => self.font_b.en = b,
            "font_b_addr" => self.font_b.addr = v32,
            "trace_valid" => self.trace.valid = b,
            "trace_trap" => self.trace.trap = b,
            "trace_intr" => self.trace.intr = b,
            "trace_cause" => self.trace.cause = v as u8,
            "trace_pc" => self.trace.pc = v32,
            "trace_insn" => self.trace.insn = v32,
            "trace_next_pc" => self.trace.next_pc = v32,
            "trace_mode" => self.trace.mode = v as u8,
            "trace_rd_we" => self.trace.rd_we = b,
            "trace_rd" => self.trace.rd = v as u8,
            "trace_rd_wdata" => self.trace.rd_wdata = v32,
            _ => panic!("unknown output port {name}"),
        }
    }
}

impl SocIn {
    /// Values in `SOC_INPUTS` order.
    pub fn to_array(&self) -> [u64; 12] {
        [
            self.rst_in as u64, self.uart_rx as u64, self.btn as u64, self.spi_miso as u64,
            self.rom_a_rdata as u64, self.rom_b_rdata as u64, self.ram_a_rdata as u64, self.ram_b_rdata as u64,
            self.vram_a_rdata as u64, self.vram_b_rdata as u64, self.font_a_rdata as u64, self.font_b_rdata as u64,
        ]
    }
}

/// Setters in `SOC_OUTPUTS` order, for engines that move values by index.
pub fn output_setters() -> Vec<fn(&mut SocOut, u64)> {
    macro_rules! setters {
        ($($name:literal),* $(,)?) => { vec![$(|o: &mut SocOut, v: u64| o.set($name, v)),*] };
    }
    setters!(
        "uart_tx", "led", "spi_cs_n", "spi_sck", "spi_mosi", "vid_r", "vid_g", "vid_b", "vid_hs", "vid_vs",
        "vid_de", "halted", "halt_code", "rom_a_en", "rom_a_addr", "rom_b_en", "rom_b_addr", "ram_a_en",
        "ram_a_addr", "ram_b_en", "ram_b_we", "ram_b_addr", "ram_b_wdata", "vram_a_en", "vram_a_we",
        "vram_a_addr", "vram_a_wdata", "vram_b_en", "vram_b_addr", "font_a_en", "font_a_we", "font_a_addr",
        "font_a_wdata", "font_b_en", "font_b_addr", "trace_valid", "trace_trap", "trace_intr", "trace_cause",
        "trace_pc", "trace_insn", "trace_next_pc", "trace_mode", "trace_rd_we", "trace_rd", "trace_rd_wdata",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_accessors_match_names() {
        let i = SocIn { btn: 5, ram_b_rdata: 7, font_b_rdata: 9, ..Default::default() };
        let a = i.to_array();
        for (k, (name, _)) in SOC_INPUTS.iter().enumerate() {
            assert_eq!(a[k], i.get(name), "{name}");
        }
        let setters = output_setters();
        assert_eq!(setters.len(), SOC_OUTPUTS.len());
        for (k, (name, w)) in SOC_OUTPUTS.iter().enumerate() {
            let v = if *w >= 32 { 0x1234_5678 } else { (1u64 << w) - 1 };
            let (mut a, mut b) = (SocOut::default(), SocOut::default());
            setters[k](&mut a, v);
            b.set(name, v);
            assert_eq!(a, b, "{name}");
        }
    }
}
