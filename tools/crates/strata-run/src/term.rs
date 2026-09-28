//! The interactive console: this terminal as the machine's serial port and,
//! on request, as its monitor.
//!
//! Console view: the machine's serial output scrolls above a status line that
//! shows the LEDs, the cycle count and the simulation speed. Screen view: the
//! video output, read back into its 80x30 text cells, fills the terminal and
//! is redrawn every frame; serial output is held and shown on return.
//!
//! Keys go to the machine's UART as typed, including Ctrl-C (which stops the
//! program running in the foreground). Ctrl-T switches views, Ctrl-] quits.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use strata_sim::devices::{TextCell, TEXT_COLS, TEXT_ROWS};

pub const KEY_QUIT: u8 = 0x1d; // Ctrl-]
pub const KEY_VIEW: u8 = 0x14; // Ctrl-T

/// CP437 code points as Unicode.
const CP437: [char; 256] = [
    ' ', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼', //
    '►', '◄', '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼', //
    ' ', '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/', //
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?', //
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', //
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', '\\', ']', '^', '_', //
    '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', //
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '{', '|', '}', '~', '⌂', //
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', ' ', //
];

/// Nearest xterm 256-color index of an RGB444 color.
fn color256(c: u16) -> u8 {
    let (r, g, b) = (((c >> 8) & 15) as u32, ((c >> 4) & 15) as u32, (c & 15) as u32);
    let six = |v: u32| ((v * 5 + 7) / 15) as u8;
    if r == g && g == b {
        // Gray ramp (232-255) or the cube's black and white.
        return match r {
            0 => 16,
            15 => 231,
            v => 232 + ((v * 23 + 7) / 15).min(23) as u8,
        };
    }
    16 + 36 * six(r) + 6 * six(g) + six(b)
}

/// Terminal settings: character mode on entry, restored when dropped.
struct CharacterMode {
    saved: Option<String>,
}

fn stty(args: &[&str]) -> Option<String> {
    Command::new("stty")
        .args(args)
        .stdin(Stdio::inherit())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

impl CharacterMode {
    fn enter() -> CharacterMode {
        let saved = stty(&["-g"]);
        if saved.is_some() {
            stty(&["-icanon", "-echo", "-isig", "min", "1"]);
        }
        CharacterMode { saved }
    }
}

impl Drop for CharacterMode {
    fn drop(&mut self) {
        if let Some(s) = &self.saved {
            stty(&[s]);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Esc {
    Normal,
    Escape,
    Csi,
}

pub struct Console {
    _mode: CharacterMode,
    rows: u16,
    cols: u16,
    status_line: bool,
    screen: bool,
    held: Vec<u8>,
    esc: Esc,
    last_status: Instant,
    shown_leds: Option<u8>,
    shown_frame: u64,
    rate: f64,
    last_rate: (Instant, u64),
    out: std::io::Stdout,
}

pub struct Snapshot<'a> {
    pub cycle: u64,
    pub leds: u8,
    pub frames: u64,
    pub cells: &'a dyn Fn() -> Vec<TextCell>,
}

impl Console {
    pub fn start(gate: bool, screen: bool) -> Console {
        let mode = CharacterMode::enter();
        let size = stty(&["size"]).and_then(|s| {
            let mut it = s.split_whitespace().map(|v| v.parse::<u16>().ok());
            Some((it.next()??, it.next()??))
        });
        let (rows, cols) = size.unwrap_or((0, 0));
        let mut c = Console {
            _mode: mode,
            rows,
            cols,
            status_line: rows >= 4,
            screen: false,
            held: Vec::new(),
            esc: Esc::Normal,
            last_status: Instant::now() - Duration::from_secs(1),
            shown_leds: None,
            shown_frame: u64::MAX,
            rate: 0.0,
            last_rate: (Instant::now(), 0),
            out: std::io::stdout(),
        };
        let mut s = String::new();
        s += "[strata-run] this terminal is the machine's serial port. Keys go to the machine;\n";
        s += "[strata-run] Ctrl-C stops the program in the foreground, Ctrl-T shows the video screen, Ctrl-] quits.\n";
        if gate {
            s += "[strata-run] the gate-level model boots in about a minute; the prompt is strata>\n";
        }
        if c.status_line {
            // Keep the last row for the status line: scroll only the rows above.
            s += &format!("\n\x1b[1A\x1b7\x1b[1;{}r\x1b8", c.rows - 1);
        }
        c.write(s.as_bytes());
        if screen {
            c.set_screen(true);
        }
        c
    }

    fn write(&mut self, b: &[u8]) {
        let _ = self.out.write_all(b);
        let _ = self.out.flush();
    }

    /// Serial output from the machine.
    pub fn serial(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.screen {
            self.held.extend_from_slice(bytes);
            return;
        }
        for &b in bytes {
            self.esc = match (self.esc, b) {
                (_, 0x1b) => Esc::Escape,
                (Esc::Escape, b'[') => Esc::Csi,
                (Esc::Csi, 0x40..=0x7e) | (Esc::Escape, _) => Esc::Normal,
                (s, _) => s,
            };
        }
        self.write(bytes);
    }

    pub fn toggle(&mut self) {
        let on = !self.screen;
        self.set_screen(on);
    }

    fn set_screen(&mut self, on: bool) {
        if on == self.screen {
            return;
        }
        self.screen = on;
        if on {
            // Alternate screen, no cursor, no scroll region.
            self.write(b"\x1b[?1049h\x1b[?25l\x1b[r\x1b[2J");
            self.shown_frame = u64::MAX;
        } else {
            let mut s = String::from("\x1b[0m\x1b[2J\x1b[?25h\x1b[?1049l");
            if self.status_line {
                s += &format!("\x1b7\x1b[1;{}r\x1b8", self.rows - 1);
            }
            self.write(s.as_bytes());
            let held = std::mem::take(&mut self.held);
            self.esc = Esc::Normal;
            self.serial(&held);
        }
        self.shown_leds = None;
        self.last_status = Instant::now() - Duration::from_secs(1);
    }

    /// Updates the screen view (on a new frame) and the status line.
    pub fn update(&mut self, snap: &Snapshot) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_rate.0).as_secs_f64();
        if dt >= 0.5 {
            self.rate = (snap.cycle - self.last_rate.1) as f64 / dt;
            self.last_rate = (now, snap.cycle);
        }
        if self.screen && snap.frames != self.shown_frame {
            self.shown_frame = snap.frames;
            self.draw_screen(&(snap.cells)());
        }
        let due = now.duration_since(self.last_status) >= Duration::from_millis(250) || self.shown_leds != Some(snap.leds);
        // Never interrupt an escape sequence of the machine's output.
        if self.status_line && due && (self.screen || self.esc == Esc::Normal) {
            self.last_status = now;
            self.shown_leds = Some(snap.leds);
            self.draw_status(snap);
        }
    }

    fn draw_screen(&mut self, cells: &[TextCell]) {
        let rows = TEXT_ROWS.min(self.rows.saturating_sub(1).max(1) as usize);
        let cols = TEXT_COLS.min(self.cols.max(1) as usize);
        let mut s = String::with_capacity(rows * cols * 12);
        let mut attr: Option<(u8, u8)> = None;
        for r in 0..rows {
            s += &format!("\x1b[{};1H", r + 1);
            for c in 0..cols {
                let cell = cells[r * TEXT_COLS + c];
                let a = (color256(cell.fg), color256(cell.bg));
                if attr != Some(a) {
                    s += &format!("\x1b[38;5;{};48;5;{}m", a.0, a.1);
                    attr = Some(a);
                }
                s.push(CP437[cell.ch as usize]);
            }
        }
        s += "\x1b[0m";
        self.write(s.as_bytes());
    }

    fn draw_status(&mut self, snap: &Snapshot) {
        let leds: String = (0..8).rev().map(|i| if snap.leds >> i & 1 == 1 { '●' } else { '○' }).collect();
        let speed = if self.rate >= 1e6 { format!("{:.1} M cycles/s", self.rate / 1e6) } else { format!("{:.0} k cycles/s", self.rate / 1e3) };
        let keys = if self.screen { "Ctrl-T: serial console" } else { "Ctrl-T: video screen" };
        let small = if self.screen && (self.rows < 31 || self.cols < 80) { "  (terminal smaller than 80x31: screen cropped)" } else { "" };
        let text = format!(" Strata  LEDs {leds}  cycle {}  {speed}  {keys}  Ctrl-C: stop program  Ctrl-]: quit{small} ", group(snap.cycle));
        let text: String = text.chars().take(self.cols as usize).collect();
        let s = format!("\x1b7\x1b[{};1H\x1b[0m\x1b[2K\x1b[7m{text}\x1b[0m\x1b8", self.rows);
        self.write(s.as_bytes());
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        if self.screen {
            self.set_screen(false);
        }
        if self.status_line {
            let s = format!("\x1b7\x1b[r\x1b[{};1H\x1b[2K\x1b8\x1b[0m\n", self.rows);
            self.write(s.as_bytes());
        }
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_numbers() {
        assert_eq!(color256(0x000), 16);
        assert_eq!(color256(0xFFF), 231);
        assert_eq!(color256(0xF00), 196);
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(CP437[0xDB], '█');
        assert_eq!(CP437[0xCD], '═');
    }
}
