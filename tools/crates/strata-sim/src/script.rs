//! Scripted interaction with the machine's terminal and buttons, for
//! reproducible whole-machine sessions.
//!
//! A script is a list of steps, one per line:
//!
//! ```text
//! # comment
//! expect strata>          wait until the terminal output contains this text
//! send ls\n               type text (\n, \t, \\, \xHH escapes)
//! wait 100000             let this many cycles pass
//! buttons 0x10            set the button levels
//! press 0x10 200000       hold buttons for a number of cycles, then release
//! screenshot shot.png     save the next complete video frame (PNG)
//! ```
//!
//! `expect` matches the text as a terminal would display it (ANSI escape
//! sequences removed) and only looks at output that arrived after the
//! previous `expect`.

use crate::engine::Engine;
use crate::machine::Machine;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Expect(String),
    Send(Vec<u8>),
    Wait(u64),
    Buttons(u8),
    Press(u8, u64),
    Screenshot(String),
}

/// Removes ANSI escape sequences (ESC [ ... final byte) from terminal output.
pub fn strip_ansi(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1B && bytes.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                i += 1;
            }
            i += 1;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

pub fn unescape(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            i += 1;
            match b[i] {
                b'n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                b't' => out.push(b'\t'),
                b'e' => out.push(0x1B),
                b'\\' => out.push(b'\\'),
                b's' => out.push(b' '),
                b'x' if i + 2 < b.len() => {
                    let h = std::str::from_utf8(&b[i + 1..i + 3]).map_err(|e| e.to_string())?;
                    out.push(u8::from_str_radix(h, 16).map_err(|_| format!("bad escape \\x{h}"))?);
                    i += 2;
                }
                c => return Err(format!("bad escape \\{}", c as char)),
            }
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    Ok(out)
}

/// Length of an ANSI escape sequence at the start of `b` (0 if none).
fn strip_ansi_prefix_len(b: &[u8]) -> usize {
    if b.first() == Some(&0x1B) && b.get(1) == Some(&b'[') {
        let mut i = 2;
        while i < b.len() && !(0x40..=0x7E).contains(&b[i]) {
            i += 1;
        }
        (i + 1).min(b.len())
    } else {
        0
    }
}

fn num(s: &str) -> Result<u64, String> {
    let r = match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|_| format!("bad number {s}"))
}

pub fn parse(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
        let err = |e: String| format!("script line {}: {e}", n + 1);
        let step = match cmd {
            "expect" => Step::Expect(String::from_utf8_lossy(&unescape(arg).map_err(err)?).into_owned()),
            "send" => Step::Send(unescape(arg).map_err(err)?),
            "wait" => Step::Wait(num(arg.trim()).map_err(err)?),
            "buttons" => Step::Buttons(num(arg.trim()).map_err(err)? as u8),
            "press" => {
                let mut it = arg.split_whitespace();
                let mask = num(it.next().unwrap_or("")).map_err(err)? as u8;
                let cycles = num(it.next().unwrap_or("")).map_err(err)?;
                Step::Press(mask, cycles)
            }
            "screenshot" => Step::Screenshot(arg.trim().to_string()),
            other => return Err(err(format!("unknown command '{other}'"))),
        };
        steps.push(step);
    }
    Ok(steps)
}

/// Runs a script. `on_step` is called for every simulated cycle, like
/// `Machine::run`. Returns an error if an `expect` is not met within
/// `timeout` cycles or the machine halts first.
pub fn run<E: Engine>(
    m: &mut Machine<E>,
    steps: &[Step],
    timeout: u64,
    mut on_cycle: impl FnMut(&Machine<E>, &crate::machine::StepEvents),
) -> Result<(), String> {
    let mut seen = 0usize; // terminal output already consumed by expects
    for step in steps {
        match step {
            Step::Expect(text) => {
                let start = m.cycle();
                loop {
                    // Search the displayed text; consume raw bytes up to the match.
                    let raw = &m.host.uart.received[seen..];
                    let hay = strip_ansi(raw);
                    if let Some(pos) = hay.find(text.as_str()) {
                        let want = pos + text.len();
                        let (mut shown, mut used) = (0, 0);
                        while shown < want && used < raw.len() {
                            used += strip_ansi_prefix_len(&raw[used..]);
                            if used < raw.len() {
                                used += 1;
                                shown += 1;
                            }
                        }
                        seen += used;
                        break;
                    }
                    if m.halted() {
                        return Err(format!("machine halted while expecting {text:?}"));
                    }
                    if m.cycle() - start > timeout {
                        return Err(format!("timed out after {timeout} cycles expecting {text:?}"));
                    }
                    let ev = m.step();
                    on_cycle(m, &ev);
                }
            }
            Step::Send(bytes) => m.host.uart.send(bytes),
            Step::Wait(n) => {
                for _ in 0..*n {
                    let ev = m.step();
                    on_cycle(m, &ev);
                    if m.halted() {
                        break;
                    }
                }
            }
            Step::Buttons(mask) => m.host.buttons = *mask,
            Step::Press(mask, n) => {
                m.host.buttons |= mask;
                for _ in 0..*n {
                    let ev = m.step();
                    on_cycle(m, &ev);
                }
                m.host.buttons &= !mask;
            }
            Step::Screenshot(path) => {
                // A frame that starts after this point: finish the current one first.
                for _ in 0..2 {
                    loop {
                        let ev = m.step();
                        on_cycle(m, &ev);
                        if ev.frame_done {
                            break;
                        }
                    }
                }
                std::fs::write(path, m.host.display.png()).map_err(|e| format!("{path}: {e}"))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escapes() {
        assert_eq!(strip_ansi(b"\x1b[92mstrata\x1b[0m> "), "strata> ");
    }

    #[test]
    fn parses_steps() {
        let s = parse("# c\nexpect strata> \nsend ls\\n\nwait 10\npress 0x10 5\n").unwrap();
        assert_eq!(
            s,
            vec![Step::Expect("strata> ".into()), Step::Send(b"ls\n".to_vec()), Step::Wait(10), Step::Press(0x10, 5)]
        );
    }
}
