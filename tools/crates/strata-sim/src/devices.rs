//! Host-side models of the devices attached to the machine's pins. Each one
//! sees only pin values, cycle by cycle, like the real part on a board.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------ UART

/// A serial terminal on the UART pins (8N1, fixed bit period in cycles).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UartTerminal {
    pub div: u32,
    /// Bytes received from the machine.
    pub received: Vec<u8>,
    // receiver (machine TX -> host)
    rx_state: u8, // 0 idle, 1 start, 2 data, 3 stop
    rx_cnt: u32,
    rx_bits: u8,
    rx_byte: u8,
    last_tx: bool,
    // transmitter (host -> machine RX)
    queue: VecDeque<u8>,
    tx_frame: u16,
    tx_bits_left: u8,
    tx_cnt: u32,
    /// Idle cycles to leave between bytes sent to the machine.
    pub gap: u32,
    gap_left: u32,
}

impl UartTerminal {
    pub fn new(div: u32) -> UartTerminal {
        UartTerminal {
            div,
            received: Vec::new(),
            rx_state: 0,
            rx_cnt: 0,
            rx_bits: 0,
            rx_byte: 0,
            last_tx: true,
            queue: VecDeque::new(),
            tx_frame: 0,
            tx_bits_left: 0,
            tx_cnt: 0,
            gap: 0,
            gap_left: 0,
        }
    }

    /// Queues bytes to send to the machine.
    pub fn send(&mut self, bytes: &[u8]) {
        self.queue.extend(bytes.iter().copied());
    }

    pub fn pending_input(&self) -> usize {
        self.queue.len() + (self.tx_bits_left > 0) as usize
    }

    /// Observes the machine's TX pin for one cycle; returns a completed byte.
    pub fn observe_tx(&mut self, tx: bool) -> Option<u8> {
        let mut out = None;
        match self.rx_state {
            0 => {
                if self.last_tx && !tx {
                    self.rx_state = 1;
                    self.rx_cnt = self.div / 2;
                }
            }
            1 => {
                if self.rx_cnt == 0 {
                    if tx {
                        self.rx_state = 0; // glitch
                    } else {
                        self.rx_state = 2;
                        self.rx_cnt = self.div - 1;
                        self.rx_bits = 0;
                    }
                } else {
                    self.rx_cnt -= 1;
                }
            }
            2 => {
                if self.rx_cnt == 0 {
                    self.rx_byte = (self.rx_byte >> 1) | ((tx as u8) << 7);
                    self.rx_bits += 1;
                    self.rx_cnt = self.div - 1;
                    if self.rx_bits == 8 {
                        self.rx_state = 3;
                    }
                } else {
                    self.rx_cnt -= 1;
                }
            }
            _ => {
                if self.rx_cnt == 0 {
                    if tx {
                        self.received.push(self.rx_byte);
                        out = Some(self.rx_byte);
                    }
                    self.rx_state = 0;
                } else {
                    self.rx_cnt -= 1;
                }
            }
        }
        self.last_tx = tx;
        out
    }

    /// Level of the machine's RX pin for the next cycle.
    pub fn drive_rx(&mut self) -> bool {
        if self.tx_bits_left == 0 {
            if self.gap_left > 0 {
                self.gap_left -= 1;
                return true;
            }
            let Some(b) = self.queue.pop_front() else { return true };
            // start (0), 8 data bits LSB first, stop (1)
            self.tx_frame = ((b as u16) << 1) | (1 << 9);
            self.tx_bits_left = 10;
            self.tx_cnt = self.div;
        }
        let level = self.tx_frame & 1 == 1;
        self.tx_cnt -= 1;
        if self.tx_cnt == 0 {
            self.tx_frame >>= 1;
            self.tx_bits_left -= 1;
            self.tx_cnt = self.div;
            if self.tx_bits_left == 0 {
                self.gap_left = self.gap;
            }
        }
        level
    }
}

// ------------------------------------------------------------------ SPI flash

/// A SPI NOR flash chip (mode 0). Supports READ (0x03, 24-bit address),
/// READ JEDEC ID (0x9F) and RELEASE POWER-DOWN (0xAB). Erased bytes read 0xFF.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiFlash {
    pub data: Vec<u8>,
    state: u8, // 0 cmd, 1 addr, 2 data, 3 id, 4 ignore
    shift_in: u8,
    nbits: u8,
    nbytes: u8,
    addr: u32,
    out_byte: u8,
    miso: bool,
    last_sck: bool,
    last_cs_n: bool,
}

impl SpiFlash {
    pub fn new(size: usize) -> SpiFlash {
        SpiFlash {
            data: vec![0xFF; size],
            state: 0,
            shift_in: 0,
            nbits: 0,
            nbytes: 0,
            addr: 0,
            out_byte: 0,
            miso: false,
            last_sck: false,
            last_cs_n: true,
        }
    }

    pub fn program(&mut self, offset: usize, bytes: &[u8]) {
        let end = (offset + bytes.len()).min(self.data.len());
        self.data[offset..end].copy_from_slice(&bytes[..end - offset]);
    }

    /// Responds to the pins observed in this cycle and returns MISO. Like the
    /// real chip, the output changes within the cycle in which SCK falls.
    pub fn respond(&mut self, cs_n: bool, sck: bool, mosi: bool) -> bool {
        if cs_n {
            self.state = 0;
            self.nbits = 0;
        } else if self.last_cs_n {
            self.state = 0;
            self.nbits = 0;
            self.nbytes = 0;
        }
        if !cs_n && !self.last_cs_n {
            if sck && !self.last_sck {
                self.shift_in = (self.shift_in << 1) | mosi as u8;
                self.nbits += 1;
                if self.nbits == 8 {
                    self.nbits = 0;
                    let b = self.shift_in;
                    match self.state {
                        0 => match b {
                            0x03 => {
                                self.state = 1;
                                self.nbytes = 0;
                                self.addr = 0;
                            }
                            0x9F => {
                                self.state = 3;
                                self.nbytes = 0;
                                self.out_byte = 0xEF;
                            }
                            _ => self.state = 4,
                        },
                        1 => {
                            self.addr = (self.addr << 8) | b as u32;
                            self.nbytes += 1;
                            if self.nbytes == 3 {
                                self.state = 2;
                                self.out_byte = self.read(self.addr);
                                self.addr += 1;
                            }
                        }
                        2 => {
                            self.out_byte = self.read(self.addr);
                            self.addr += 1;
                        }
                        3 => {
                            self.nbytes += 1;
                            self.out_byte = if self.nbytes == 1 { 0x40 } else { 0x18 };
                        }
                        _ => {}
                    }
                }
            } else if !sck && self.last_sck && (self.state == 2 || self.state == 3) {
                self.miso = (self.out_byte >> (7 - self.nbits)) & 1 == 1;
            }
        }
        self.last_sck = sck;
        self.last_cs_n = cs_n;
        self.miso
    }

    fn read(&self, addr: u32) -> u8 {
        self.data.get((addr & 0xFF_FFFF) as usize).copied().unwrap_or(0xFF)
    }
}

// ------------------------------------------------------------------ video

pub const VIDEO_W: usize = 640;
pub const VIDEO_H: usize = 480;

/// A display that captures pixels from the video pins, positioning them by
/// the sync and display-enable signals only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Display {
    /// Frame being drawn (RGB444).
    pub current: Vec<u16>,
    /// Last complete frame.
    pub frame: Vec<u16>,
    pub frames: u64,
    x: usize,
    y: usize,
    last_de: bool,
    last_vs: bool,
    saw_line: bool,
}

impl Default for Display {
    fn default() -> Self {
        Display {
            current: vec![0; VIDEO_W * VIDEO_H],
            frame: vec![0; VIDEO_W * VIDEO_H],
            frames: 0,
            x: 0,
            y: 0,
            last_de: false,
            last_vs: true,
            saw_line: false,
        }
    }
}

impl Display {
    /// Returns true when a frame completes (start of vertical sync).
    pub fn observe(&mut self, rgb: u16, hs: bool, vs: bool, de: bool) -> bool {
        let _ = hs;
        let mut done = false;
        if de {
            if self.x < VIDEO_W && self.y < VIDEO_H {
                self.current[self.y * VIDEO_W + self.x] = rgb;
            }
            self.x += 1;
            self.saw_line = true;
        } else if self.last_de {
            self.x = 0;
            self.y += 1;
        }
        if self.last_vs && !vs {
            if self.saw_line {
                self.frame.copy_from_slice(&self.current);
                self.frames += 1;
                done = true;
            }
            self.x = 0;
            self.y = 0;
            self.saw_line = false;
        }
        self.last_de = de;
        self.last_vs = vs;
        done
    }

    /// The last complete frame as 8-bit RGB.
    pub fn rgb8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(VIDEO_W * VIDEO_H * 3);
        for &p in &self.frame {
            for sh in [8, 4, 0] {
                let c = ((p >> sh) & 0xF) as u8;
                out.push(c << 4 | c);
            }
        }
        out
    }

    /// The last complete frame as a binary PPM image.
    pub fn ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{VIDEO_W} {VIDEO_H}\n255\n").into_bytes();
        out.extend(self.rgb8());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_loopback() {
        let mut t = UartTerminal::new(8);
        t.send(b"Hi\n");
        let mut got = Vec::new();
        for _ in 0..8 * 10 * 4 {
            let level = t.drive_rx();
            if let Some(b) = t.observe_tx(level) {
                got.push(b);
            }
        }
        assert_eq!(got, b"Hi\n");
    }

    #[test]
    fn flash_read_command() {
        let mut f = SpiFlash::new(1024);
        f.program(0x10, &[0xA5, 0x3C]);
        let clock_byte = |f: &mut SpiFlash, out: u8| -> u8 {
            let mut got = 0u8;
            for i in 0..8 {
                let mosi = (out >> (7 - i)) & 1 == 1;
                let miso = f.respond(false, false, mosi); // low phase: data valid
                got = (got << 1) | miso as u8;
                f.respond(false, true, mosi); // rising edge
            }
            f.respond(false, false, false); // falling edge
            got
        };
        f.respond(true, false, false);
        f.respond(false, false, false);
        for b in [0x03, 0x00, 0x00, 0x10] {
            clock_byte(&mut f, b);
        }
        assert_eq!(clock_byte(&mut f, 0), 0xA5);
        assert_eq!(clock_byte(&mut f, 0), 0x3C);
    }
}
