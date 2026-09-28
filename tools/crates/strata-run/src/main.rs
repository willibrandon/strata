//! strata-run: runs the Strata machine without the debugger.
//!
//! In gate mode the machine is the synthesized sky130 netlist evaluated cell
//! by cell; the only host-side parts are the memory macros and the devices on
//! the pins (terminal, boot flash, buttons, display).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{channel, TryRecvError};
use std::time::Instant;

mod term;

use strata_gate::{EvalMode, GateSim, PowerOn};
use strata_rtl::RtlSim;
use strata_sim::machine::FLASH_IMAGE_OFFSET;
use strata_sim::{Engine, Lockstep, Machine, MachineConfig};

const USAGE: &str = "usage: strata-run [options]
  --engine gate|rtl|lockstep
                         gate: sky130 netlist (default); rtl: Verilator model of
                         the RTL; lockstep: both, comparing every output every cycle
  --netlist <json>       gate-level netlist (default build/gate/strata_soc.json)
  --liberty <lib>        cell library (default: $PDK_ROOT/sky130A/.../tt_025C_1v80.lib)
  --rom <hex|bin>        boot ROM image (default build/sw/boot/boot.hex)
  --ram <hex|bin>        preload RAM (test-harness shortcut)
  --flash <file>|none    flash contents from 0x200000 (default build/sw/flash.bin,
                         unless --ram is given; none: erased flash)
  -i, --interactive      use this terminal as the machine's console: keys go to
                         the UART as you type (Ctrl-C stops the program in the
                         foreground), output appears as the machine sends it,
                         a status line shows the LEDs; Ctrl-T switches to the
                         video screen and back; Ctrl-] quits. No cycle limit
                         unless --max-cycles is given
  --screen               like --interactive, starting on the video screen
  --uart-in <text>       lines to type on the terminal (\\n ends a line); each
                         line is typed after the machine prints a prompt
                         ending in \"> \", as a person would (the UART has no
                         flow control)
  --uart-raw <text>      bytes to send on the serial line from cycle 0
  --script <file>        run an expect/send script (see strata-sim script.rs),
                         then continue until halt or --max-cycles
  --uart-div <n>         terminal bit period in cycles (default 25)
  --buttons <mask>       button levels (bit 0 UP ... bit 6 START)
  --max-cycles <n>       stop after n cycles (default 10000000)
  --trace <file>         write the retirement trace
  --screenshot <file>    save the last complete video frame (.png or .ppm)
  --frame <n>            stop when video frame n is complete (with --screenshot,
                         saves exactly that frame)
  --eval event|full      combinational evaluation strategy (default event)
  --power-on zero|<seed> initial flip-flop contents (default zero)
  --expect-halt <code>   exit status 0 only if the machine halts with this code
  --quiet                do not echo UART output";

#[derive(Clone, Copy, PartialEq)]
enum EngineKind {
    Gate,
    Rtl,
    Lockstep,
}

struct Args {
    engine: EngineKind,
    netlist: PathBuf,
    liberty: PathBuf,
    rom: Option<PathBuf>,
    ram: Option<PathBuf>,
    flash: Option<PathBuf>,
    erased_flash: bool,
    uart_in: Option<String>,
    uart_raw: Option<String>,
    script: Option<PathBuf>,
    uart_div: u32,
    buttons: u8,
    max_cycles: u64,
    trace: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    frame: Option<u64>,
    interactive: bool,
    screen: bool,
    max_cycles_set: bool,
    eval: EvalMode,
    power_on: PowerOn,
    expect_halt: Option<u8>,
    quiet: bool,
}

fn default_liberty() -> PathBuf {
    let root = std::env::var("PDK_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("tools/pdk"));
    root.join("sky130A/libs.ref/sky130_fd_sc_hd/lib/sky130_fd_sc_hd__tt_025C_1v80.lib")
}

fn parse_num(s: &str) -> Result<u64, String> {
    let r = if let Some(h) = s.strip_prefix("0x") { u64::from_str_radix(h, 16) } else { s.parse() };
    r.map_err(|_| format!("bad number {s}"))
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        engine: EngineKind::Gate,
        netlist: PathBuf::from("build/gate/strata_soc.json"),
        liberty: default_liberty(),
        rom: None,
        ram: None,
        flash: None,
        erased_flash: false,
        uart_in: None,
        uart_raw: None,
        script: None,
        uart_div: 25,
        buttons: 0,
        max_cycles: 10_000_000,
        trace: None,
        screenshot: None,
        frame: None,
        interactive: false,
        screen: false,
        max_cycles_set: false,
        eval: EvalMode::Event,
        power_on: PowerOn::Zero,
        expect_halt: None,
        quiet: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--engine" => {
                a.engine = match val()?.as_str() {
                    "gate" => EngineKind::Gate,
                    "rtl" => EngineKind::Rtl,
                    "lockstep" => EngineKind::Lockstep,
                    o => return Err(format!("bad --engine {o}")),
                }
            }
            "--netlist" => a.netlist = val()?.into(),
            "--liberty" => a.liberty = val()?.into(),
            "--rom" => a.rom = Some(val()?.into()),
            "--ram" => a.ram = Some(val()?.into()),
            "--flash" => {
                let v = val()?;
                if v == "none" {
                    a.erased_flash = true;
                } else {
                    a.flash = Some(v.into());
                }
            }
            "--uart-in" => a.uart_in = Some(val()?.replace("\\n", "\n")),
            "--uart-raw" => a.uart_raw = Some(val()?.replace("\\n", "\n")),
            "--script" => a.script = Some(val()?.into()),
            "--uart-div" => a.uart_div = parse_num(&val()?)? as u32,
            "--buttons" => a.buttons = parse_num(&val()?)? as u8,
            "--max-cycles" => {
                a.max_cycles = parse_num(&val()?)?;
                a.max_cycles_set = true;
            }
            "--trace" => a.trace = Some(val()?.into()),
            "--screenshot" => a.screenshot = Some(val()?.into()),
            "-i" | "--interactive" => a.interactive = true,
            "--screen" => {
                a.interactive = true;
                a.screen = true;
            }
            "--frame" => a.frame = Some(parse_num(&val()?)?),
            "--eval" => {
                a.eval = match val()?.as_str() {
                    "event" => EvalMode::Event,
                    "full" => EvalMode::Full,
                    o => return Err(format!("bad --eval {o}")),
                }
            }
            "--power-on" => {
                let v = val()?;
                a.power_on = if v == "zero" { PowerOn::Zero } else { PowerOn::Random(parse_num(&v)?) };
            }
            "--expect-halt" => a.expect_halt = Some(parse_num(&val()?)? as u8),
            "--quiet" => a.quiet = true,
            "-h" | "--help" => return Err(USAGE.into()),
            o => return Err(format!("unknown option {o}\n{USAGE}")),
        }
    }
    // Default to the software build: boot from the ROM and the flash image.
    let existing = |p: &str| Some(PathBuf::from(p)).filter(|p| p.exists());
    if a.rom.is_none() {
        a.rom = existing("build/sw/boot/boot.hex");
    }
    if a.flash.is_none() && a.ram.is_none() && !a.erased_flash {
        a.flash = existing("build/sw/flash.bin");
    }
    Ok(a)
}

/// Runs the machine with this terminal as its console (term.rs) until it
/// halts, the user quits, or `limit` cycles. Returns whether it halted.
fn interactive<E: Engine>(
    m: &mut Machine<E>,
    observe: &mut dyn FnMut(&strata_sim::StepEvents),
    limit: Option<u64>,
    gate: bool,
    screen: bool,
) -> bool {
    let mut con = term::Console::start(gate, screen);
    let (tx, rx) = channel::<u8>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 64];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 || buf[..n].iter().any(|&b| tx.send(b).is_err()) {
                break;
            }
        }
    });
    let mut shown = m.host.uart.received.len();
    loop {
        loop {
            match rx.try_recv() {
                Ok(term::KEY_QUIT) => return false,
                Ok(term::KEY_VIEW) => con.toggle(),
                Ok(b) => m.host.uart.send(&[b]),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        // Short slices keep typing responsive on the slower gate-level model.
        let halted = m.run(2000, &mut *observe);
        if halted {
            m.drain_uart(1_000_000);
        }
        con.serial(&m.host.uart.received[shown..]);
        shown = m.host.uart.received.len();
        let (font, display) = (&m.host.mem.font.words, &m.host.display);
        con.update(&term::Snapshot {
            cycle: m.host.cycle,
            leds: m.last_out.led,
            frames: display.frames,
            cells: &|| display.text_cells(font),
        });
        if halted {
            return true;
        }
        if limit.is_some_and(|l| m.cycle() >= l) {
            return false;
        }
    }
}

/// Reads a memory image: `$readmemh`-style hex words (one per line) or raw bytes.
fn read_image(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if path.extension().is_some_and(|e| e == "hex") {
        let text = String::from_utf8_lossy(&bytes);
        let mut out = Vec::new();
        for tok in text.split_whitespace() {
            if tok.starts_with("//") || tok.starts_with('@') {
                continue;
            }
            let w = u32::from_str_radix(tok, 16).map_err(|_| format!("{}: bad hex word {tok}", path.display()))?;
            out.extend_from_slice(&w.to_le_bytes());
        }
        Ok(out)
    } else {
        Ok(bytes)
    }
}

/// Engine-specific report printed after the run.
trait Report {
    fn report(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Report for GateSim {
    fn report(&self) -> Result<(), String> {
        let s = &self.stats;
        eprintln!("[strata-run] gate: {:.0} node evaluations per cycle", s.node_evals as f64 / s.cycles.max(1) as f64);
        Ok(())
    }
}

impl Report for RtlSim {}

impl Report for Lockstep<RtlSim, GateSim> {
    fn report(&self) -> Result<(), String> {
        match &self.mismatch {
            None => {
                eprintln!("[strata-run] lockstep: RTL and gate outputs identical for all {} cycles", self.cycle);
                Ok(())
            }
            Some(m) => Err(format!("lockstep mismatch\n{}", m.describe())),
        }
    }
}

fn gate_engine(args: &Args) -> Result<GateSim, String> {
    let t0 = Instant::now();
    let compiled = strata_gate::load(&args.netlist, &args.liberty, "strata_soc")?;
    let mut engine = GateSim::new(compiled, args.power_on)?;
    engine.mode = args.eval;
    eprintln!("[strata-run] loaded netlist in {:.2?}", t0.elapsed());
    Ok(engine)
}

fn run() -> Result<ExitCode, String> {
    let args = parse_args()?;
    match args.engine {
        EngineKind::Gate => simulate(gate_engine(&args)?, &args),
        EngineKind::Rtl => simulate(RtlSim::new()?, &args),
        EngineKind::Lockstep => {
            let config = MachineConfig::default();
            let skip = config.reset_cycles + strata_sim::machine::RESET_LATENCY;
            simulate(Lockstep::new(RtlSim::new()?, gate_engine(&args)?, skip), &args)
        }
    }
}

fn simulate<E: Engine + Report>(engine: E, args: &Args) -> Result<ExitCode, String> {
    eprintln!("[strata-run] {}", engine.describe());
    let config = MachineConfig { uart_div: args.uart_div, ..Default::default() };
    let mut m = Machine::new(engine, config);
    if let Some(p) = &args.rom {
        m.host.mem.rom.load_bytes(&read_image(p)?);
    }
    if let Some(p) = &args.ram {
        m.host.mem.ram.load_bytes(&read_image(p)?);
    }
    if let Some(p) = &args.flash {
        m.host.flash.program(FLASH_IMAGE_OFFSET, &read_image(p)?);
    }
    if let Some(s) = &args.uart_raw {
        m.host.uart.send(s.as_bytes());
    }
    m.host.buttons = args.buttons;

    let mut trace = match &args.trace {
        Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p).map_err(|e| e.to_string())?)),
        None => None,
    };
    // The interactive console prints the serial output itself.
    let quiet = args.quiet || args.interactive;
    let mut retired = 0u64;
    let t1 = Instant::now();
    let mut observe = |ev: &strata_sim::StepEvents| {
        if let Some(r) = &ev.retire {
            if !r.trap {
                retired += 1;
            }
            if let Some(t) = &mut trace {
                let _ = writeln!(t, "{r}");
            }
        }
        if let (Some(b), false) = (ev.uart_byte, quiet) {
            let mut o = std::io::stdout();
            let _ = o.write_all(&[b]);
            let _ = o.flush();
        }
    };
    let mut steps = Vec::new();
    if let Some(text) = &args.uart_in {
        for line in text.split_inclusive('\n') {
            steps.push(strata_sim::script::Step::Expect("> ".into()));
            steps.push(strata_sim::script::Step::Send(line.as_bytes().to_vec()));
        }
    }
    if let Some(p) = &args.script {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        steps.extend(strata_sim::script::parse(&text)?);
    }
    if !steps.is_empty() {
        strata_sim::script::run(&mut m, &steps, args.max_cycles, |_, ev| observe(ev))
            .map_err(|e| format!("script: {e}"))?;
    }
    let halted = match args.frame {
        _ if args.interactive => {
            let limit = args.max_cycles_set.then_some(args.max_cycles);
            let gate = !m.engine.describe().starts_with("rtl");
            m.halted() || interactive(&mut m, &mut observe, limit, gate, args.screen)
        }
        Some(n) => {
            let mut halted = m.halted();
            while !halted && m.host.display.frames < n && m.cycle() < args.max_cycles {
                halted = m.run(1, &mut observe);
            }
            halted
        }
        None => {
            let remaining = args.max_cycles.saturating_sub(m.cycle());
            m.halted() || m.run(remaining, &mut observe)
        }
    };
    let elapsed = t1.elapsed();
    if halted {
        let before = m.host.uart.received.len();
        m.drain_uart(1_000_000);
        if !quiet {
            let _ = std::io::stdout().write_all(&m.host.uart.received[before..]);
        }
    }
    let cycles = m.cycle();
    eprintln!(
        "[strata-run] {} after {cycles} cycles, {retired} instructions retired, {:.0} cycles/s",
        if halted { format!("halted with code {}", m.last_out.halt_code) } else { "stopped".into() },
        cycles as f64 / elapsed.as_secs_f64(),
    );
    if let Some(p) = &args.screenshot {
        if args.frame.is_none() {
            // Show everything drawn so far: finish the frame in progress, then a full one.
            m.run_until_frame(1_000_000);
            m.run_until_frame(1_000_000);
        }
        let data = if p.extension().is_some_and(|e| e == "png") { m.host.display.png() } else { m.host.display.ppm() };
        std::fs::write(p, data).map_err(|e| e.to_string())?;
        eprintln!("[strata-run] wrote frame {} to {}", m.host.display.frames, p.display());
    }
    m.engine.report()?;
    Ok(match args.expect_halt {
        Some(code) if !(halted && m.last_out.halt_code == code) => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("strata-run: {e}");
            ExitCode::from(2)
        }
    }
}
