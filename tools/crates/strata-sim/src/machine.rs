//! The clock-cycle loop that connects an engine to memories and devices.

use serde::{Deserialize, Serialize};

use crate::devices::{Display, DisplaySnapshot, SpiFlash, UartTerminal};
use crate::engine::Engine;
use crate::mem::{MemAccess, Memories};
use crate::ports::{SocIn, SocOut};
use crate::retire::Retire;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineConfig {
    /// UART bit period of the attached terminal, in cycles.
    pub uart_div: u32,
    /// `rst_in` is held high for this many cycles after power-on.
    pub reset_cycles: u64,
    pub flash_bytes: usize,
}

impl Default for MachineConfig {
    fn default() -> Self {
        MachineConfig { uart_div: 25, reset_cycles: 8, flash_bytes: 4 << 20 }
    }
}

/// Offset of the boot image in the boot flash (docs/memory-map.md).
pub const FLASH_IMAGE_OFFSET: usize = 0x20_0000;

/// Cycles from `rst_in` rising until the design is in reset: the two-flop
/// synchronizer in `st_reset` plus the cycle in which reset is applied.
pub const RESET_LATENCY: u64 = 3;

/// What happened in one cycle.
#[derive(Debug, Clone, Default)]
pub struct StepEvents {
    pub retire: Option<Retire>,
    pub uart_byte: Option<u8>,
    pub frame_done: bool,
}

/// Host-side state that is not part of the design: memories and devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostState {
    pub cycle: u64,
    pub inputs: SocIn,
    pub mem: Memories,
    pub uart: UartTerminal,
    pub flash: SpiFlash,
    pub display: Display,
    pub buttons: u8,
    pub reset_until: u64,
}

/// Host state with the display compressed, for checkpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSnapshot {
    pub cycle: u64,
    pub inputs: SocIn,
    pub mem: Memories,
    pub uart: UartTerminal,
    pub flash_state: SpiFlash,
    pub display: DisplaySnapshot,
    pub buttons: u8,
    pub reset_until: u64,
    pub last_out: SocOut,
}

pub struct Machine<E: Engine> {
    pub engine: E,
    pub host: HostState,
    pub config: MachineConfig,
    /// Outputs of the last completed cycle.
    pub last_out: SocOut,
    /// Outputs of the current cycle (valid when settled).
    out: SocOut,
    settled: bool,
    /// When set, every memory access of each cycle is appended here.
    pub mem_log: Option<Vec<(u64, &'static str, MemAccess)>>,
}

impl<E: Engine> Machine<E> {
    pub fn new(engine: E, config: MachineConfig) -> Machine<E> {
        let host = HostState {
            cycle: 0,
            inputs: SocIn { uart_rx: true, rst_in: true, ..Default::default() },
            mem: Memories::default(),
            uart: UartTerminal::new(config.uart_div),
            flash: SpiFlash::new(config.flash_bytes),
            display: Display::default(),
            buttons: 0,
            reset_until: config.reset_cycles,
        };
        Machine { engine, host, config, last_out: SocOut::default(), out: SocOut::default(), settled: false, mem_log: None }
    }

    pub fn cycle(&self) -> u64 {
        self.host.cycle
    }

    pub fn halted(&self) -> bool {
        self.last_out.halted && !self.in_reset()
    }

    /// True until reset has propagated through the synchronizer. Outputs seen
    /// before then reflect power-on flip-flop contents, not the machine.
    pub fn in_reset(&self) -> bool {
        self.host.cycle <= self.host.reset_until + RESET_LATENCY
    }

    /// Asserts the external reset for `cycles` cycles starting now.
    pub fn reset(&mut self, cycles: u64) {
        self.host.reset_until = self.host.cycle + cycles;
        self.settled = false;
    }

    /// Captures all host-side state (memories, devices, cycle). The flash
    /// contents are included; they are shared by value, so keep checkpoints
    /// few or clear the flash data if it is large and constant.
    pub fn host_snapshot(&self) -> HostSnapshot {
        let mut flash = self.host.flash.clone();
        flash.data = Vec::new(); // constant; restored from the live machine
        HostSnapshot {
            cycle: self.host.cycle,
            inputs: self.host.inputs,
            mem: self.host.mem.clone(),
            uart: self.host.uart.clone(),
            flash_state: flash,
            display: self.host.display.snapshot(),
            buttons: self.host.buttons,
            reset_until: self.host.reset_until,
            last_out: self.last_out,
        }
    }

    /// Restores host state captured by `host_snapshot`. The engine must be
    /// restored separately to the same cycle.
    pub fn restore_host(&mut self, s: &HostSnapshot) {
        let data = std::mem::take(&mut self.host.flash.data);
        self.host.flash = s.flash_state.clone();
        self.host.flash.data = data;
        self.host.cycle = s.cycle;
        self.host.inputs = s.inputs;
        self.host.mem = s.mem.clone();
        self.host.uart = s.uart.clone();
        self.host.display.restore(&s.display);
        self.host.buttons = s.buttons;
        self.host.reset_until = s.reset_until;
        self.last_out = s.last_out;
        self.settled = false;
    }

    /// Marks the current cycle as needing re-evaluation (after the host state
    /// or the engine was changed from outside, e.g. a restored snapshot).
    pub fn invalidate(&mut self) {
        self.settled = false;
    }

    /// Applies the current cycle's inputs and settles combinational logic.
    /// Between steps the machine is always settled: every net and output
    /// reflects the current cycle, before its clock edge.
    fn settle(&mut self) {
        let h = &mut self.host;
        h.inputs.rst_in = h.cycle < h.reset_until;
        h.inputs.btn = h.buttons & 0x7F;
        self.engine.set_inputs(&h.inputs);
        self.engine.eval();
        let mut out = self.engine.outputs();
        // The flash drives MISO within the cycle in which it sees SCK fall.
        let miso = h.flash.respond(out.spi_cs_n, out.spi_sck, out.spi_mosi);
        if miso != h.inputs.spi_miso {
            h.inputs.spi_miso = miso;
            self.engine.set_inputs(&h.inputs);
            self.engine.eval();
            out = self.engine.outputs();
        }
        self.out = out;
        self.settled = true;
    }

    /// Outputs of the current (settled) cycle.
    pub fn outputs(&mut self) -> &SocOut {
        if !self.settled {
            self.settle();
        }
        &self.out
    }

    /// Re-evaluates the current cycle after a change made from outside
    /// (a forced net, a flipped flip-flop, new button levels).
    pub fn resettle(&mut self) {
        self.settle();
    }

    /// Completes the current cycle (observation, clock edge) and settles the
    /// next one.
    pub fn step(&mut self) -> StepEvents {
        if !self.settled {
            self.settle();
        }
        let out = self.out;
        let h = &mut self.host;
        let live = h.cycle > h.reset_until + RESET_LATENCY;
        let ev = StepEvents {
            retire: if live { Retire::from_trace(h.cycle, &out.trace) } else { None },
            uart_byte: h.uart.observe_tx(out.uart_tx),
            frame_done: h.display.observe(out.vid_rgb, out.vid_hs, out.vid_vs, out.vid_de),
        };

        // Memory macros capture their ports at the clock edge.
        if let Some(log) = &mut self.mem_log {
            let mut acc = Vec::new();
            for (name, m, a, b) in [
                ("rom", &mut h.mem.rom, &out.rom_a, &out.rom_b),
                ("ram", &mut h.mem.ram, &out.ram_a, &out.ram_b),
                ("vram", &mut h.mem.vram, &out.vram_a, &out.vram_b),
                ("font", &mut h.mem.font, &out.font_a, &out.font_b),
            ] {
                acc.clear();
                m.clock(a, b, Some(&mut acc));
                log.extend(acc.iter().map(|x| (h.cycle, name, *x)));
            }
        } else {
            h.mem.rom.clock(&out.rom_a, &out.rom_b, None);
            h.mem.ram.clock(&out.ram_a, &out.ram_b, None);
            h.mem.vram.clock(&out.vram_a, &out.vram_b, None);
            h.mem.font.clock(&out.font_a, &out.font_b, None);
        }

        self.engine.tick();

        h.inputs.uart_rx = h.uart.drive_rx();
        h.inputs.rom_a_rdata = h.mem.rom.a_rdata;
        h.inputs.rom_b_rdata = h.mem.rom.b_rdata;
        h.inputs.ram_a_rdata = h.mem.ram.a_rdata;
        h.inputs.ram_b_rdata = h.mem.ram.b_rdata;
        h.inputs.vram_a_rdata = h.mem.vram.a_rdata;
        h.inputs.vram_b_rdata = h.mem.vram.b_rdata;
        h.inputs.font_a_rdata = h.mem.font.a_rdata;
        h.inputs.font_b_rdata = h.mem.font.b_rdata;
        h.cycle += 1;
        self.last_out = out;
        self.settle();
        ev
    }

    /// Runs until the machine halts or `max_cycles` have elapsed, calling
    /// `on_event` for every cycle. Returns true if the machine halted.
    pub fn run(&mut self, max_cycles: u64, mut on_event: impl FnMut(&StepEvents)) -> bool {
        let end = self.host.cycle + max_cycles;
        while self.host.cycle < end {
            let ev = self.step();
            on_event(&ev);
            if self.halted() {
                return true;
            }
        }
        false
    }

    /// Runs until the display completes a new frame (at most `max_cycles`).
    /// Video keeps scanning after the CPU halts, so this also captures the
    /// final screen of a halted machine.
    pub fn run_until_frame(&mut self, max_cycles: u64) -> bool {
        for _ in 0..max_cycles {
            if self.step().frame_done {
                return true;
            }
        }
        false
    }

    /// After halting, runs until the UART transmitter has been idle for two
    /// frames (so the last bytes written by software reach the terminal).
    pub fn drain_uart(&mut self, max_cycles: u64) {
        let need = self.config.uart_div as u64 * 20;
        let mut idle = 0;
        for _ in 0..max_cycles {
            self.step();
            idle = if self.last_out.uart_tx { idle + 1 } else { 0 };
            if idle >= need {
                break;
            }
        }
    }
}
