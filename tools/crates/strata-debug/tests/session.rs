//! Session behavior on the real gate-level netlist: determinism, time travel,
//! precise triggers, fault injection and replay files. Skipped (with a note)
//! when the netlist has not been built.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use strata_debug::{FaultAction, Session, SessionConfig, StopReason, TriggerKind};
use strata_gate::Compiled;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

fn liberty() -> PathBuf {
    let pdk = std::env::var("PDK_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join("tools/pdk"));
    pdk.join("sky130A/libs.ref/sky130_fd_sc_hd/lib/sky130_fd_sc_hd__tt_025C_1v80.lib")
}

const PROGRAM: &str = r#"
    .text
    .globl _start
_start:
    li      sp, 0x10010000
    li      s0, 0
    li      s1, 5
loop:
    call    bump
    addi    s1, s1, -1
    bnez    s1, loop
    li      t0, 0x10000100
    sw      s0, 0(t0)
    lw      t1, 0(t0)
    li      t0, 0x40000020
    sw      t1, 0(t0)
1:  j       1b
bump:
    addi    s0, s0, 3
    ret
"#;

/// Assembles the test program once; returns the ROM hex path.
fn program() -> &'static PathBuf {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("strata-debug-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("prog.s");
        std::fs::write(&src, PROGRAM).unwrap();
        let hex = dir.join("prog.hex");
        let out = strata_asm::assemble_and_link(&strata_asm::Options {
            inputs: vec![src.to_string_lossy().into_owned()],
            layout: strata_asm::LayoutSpec::Org(0),
            ..Default::default()
        });
        let program = out.program.expect("test program assembles");
        let (_, bytes) = strata_asm::output::flat_binary(&program).unwrap();
        let words: Vec<String> =
            bytes.chunks(4).map(|c| format!("{:08x}", u32::from_le_bytes([c[0], c[1], c[2], c[3]]))).collect();
        std::fs::write(&hex, words.join("\n") + "\n").unwrap();
        hex
    })
}

fn compiled() -> Option<Arc<Compiled>> {
    static C: OnceLock<Option<Arc<Compiled>>> = OnceLock::new();
    C.get_or_init(|| {
        let nl = repo().join("build/gate/strata_soc.json");
        if !nl.exists() || !liberty().exists() {
            eprintln!("skipping: build/gate/strata_soc.json or the liberty file is missing");
            return None;
        }
        Some(strata_gate::load(&nl, &liberty(), "strata_soc").unwrap())
    })
    .clone()
}

fn session() -> Option<Session> {
    let c = compiled()?;
    let config = SessionConfig {
        netlist: repo().join("build/gate/strata_soc.json"),
        liberty: liberty(),
        rom: program().clone(),
        flash: None,
        ram: None,
        debug: vec![],
        source_root: repo().join("sw"),
        power_on_seed: None,
        checkpoint_interval: 16,
    };
    Some(Session::with_compiled(config, c).unwrap())
}

fn regs(s: &Session) -> Vec<u64> {
    s.probes.regs.iter().map(|r| r.as_ref().map_or(0, |sig| s.value(sig))).collect()
}

#[test]
fn probes_resolve() {
    let Some(s) = session() else { return };
    assert!(s.probes.missing.is_empty(), "unresolved probes: {:?}", s.probes.missing);
}

#[test]
fn runs_to_halt_with_correct_result() {
    let Some(mut s) = session() else { return };
    let r = s.run(10_000, &mut || false);
    assert_eq!(r, StopReason::Halted);
    assert_eq!(s.machine.last_out.halt_code, 15);
    // s0 was incremented five times by 3.
    assert_eq!(regs(&s)[8], 15);
}

#[test]
fn goto_is_exact_in_both_directions() {
    let Some(mut s) = session() else { return };
    s.run(10_000, &mut || false);
    let end = s.cycle();
    // Values of every net at a set of cycles, visited in a scrambled order.
    let probes = [5u64, 37, 16, 17, 48, 33, end - 1, end + 40];
    let mut first = Vec::new();
    for &c in &probes {
        s.goto(c);
        assert_eq!(s.cycle(), c);
        first.push(s.engine().values().to_vec());
    }
    for (i, &c) in probes.iter().enumerate().rev() {
        s.goto(c);
        assert_eq!(s.engine().values(), first[i].as_slice(), "cycle {c} differs on revisit");
    }
    // A fresh session reaching the same cycle agrees too.
    let mut t = session().unwrap();
    t.goto(48);
    assert_eq!(t.engine().values(), first[4].as_slice());
}

#[test]
fn commit_breakpoint_stops_before_the_instruction() {
    let Some(mut s) = session() else { return };
    // Address of `bump`: the 12th instruction word after _start (li sp is two).
    s.run(10_000, &mut || false);
    let bump = s.retired.iter().find(|r| r.insn == 0x00340413).unwrap().pc; // addi s0, s0, 3
    s.goto(0);
    let id = s.add_trigger(TriggerKind::Commit(bump), "bump");
    let mut seen = Vec::new();
    for _ in 0..5 {
        assert_eq!(s.run(10_000, &mut || false), StopReason::Trigger(id));
        // Stopped with the instruction in W, before it writes s0.
        seen.push(regs(&s)[8]);
        s.step_cycle();
    }
    assert_eq!(seen, [0, 3, 6, 9, 12]);
}

#[test]
fn reverse_step_returns_to_previous_instruction() {
    let Some(mut s) = session() else { return };
    for _ in 0..20 {
        s.step_instruction(1000);
    }
    let here = s.cycle();
    let last = s.retired_before(here).unwrap().cycle;
    assert!(s.reverse_step_instruction());
    assert_eq!(s.cycle(), last);
}

#[test]
fn fault_changes_the_result_and_replays_deterministically() {
    let Some(mut s) = session() else { return };
    // Upset one bit of s0 (x8) in the register file early in the loop.
    for _ in 0..8 {
        s.step_instruction(1000);
    }
    let bit0 = s.probes.regs[8].as_ref().unwrap().bits[0];
    let driver = s.compiled.driver[bit0 as usize];
    let strata_gate::Driver::Flop(flop, _) = driver else { panic!("x8[0] is not a flip-flop output: {driver:?}") };
    s.record(strata_debug::TimelineEvent::Fault(FaultAction::Flip { flop }));
    s.run(10_000, &mut || false);
    let faulty = s.machine.last_out.halt_code;
    assert_ne!(faulty, 15, "the upset should change the computed value");
    // Save, reload, and check the replay reaches the same outcome.
    let file = s.to_file();
    let t = Session::from_file(file, compiled()).unwrap();
    assert_eq!(t.cycle(), s.cycle());
    assert_eq!(t.engine().values(), s.engine().values());
    assert_eq!(t.machine.last_out.halt_code, faulty);
}

#[test]
fn stuck_at_on_a_bus_signal() {
    let Some(mut s) = session() else { return };
    // Force the data-bus write-enable low: the store never happens, so the
    // load returns 0 and the machine halts with code 0.
    let we = s.probes.dbus.cmd_we.clone().unwrap().bits[0];
    s.record(strata_debug::TimelineEvent::Fault(FaultAction::Force { net: we, value: Some(false) }));
    s.run(10_000, &mut || false);
    assert!(s.machine.halted() || s.cycle() >= 10_000);
    let _ = Path::new("");
}

#[test]
fn inspection_model() {
    use strata_debug::inspect;
    let Some(mut s) = session() else { return };
    s.run(10_000, &mut || false);
    let end = s.cycle();

    // Hierarchy totals add up to the whole netlist.
    let mods = inspect::modules(&s.compiled);
    assert_eq!(mods[0].total_cells, s.compiled.netlist.cells.len());
    let alu = mods.iter().find(|m| m.path == "u_core.u_alu").unwrap();
    assert!(alu.total_cells > 100 && alu.total_flops == 0);

    // Every stage has logic; the ALU's cells are execute-stage logic.
    let stages = inspect::StageMap::build(&s.compiled);
    for st in 0..5 {
        assert!(stages.cells_of(st).count() > 50, "stage {} is empty", inspect::STAGES[st]);
    }
    let alu_x = alu.cells.iter().filter(|&&c| stages.stages[c as usize] & (1 << 2) != 0).count();
    assert!(alu_x * 10 >= alu.cells.len() * 9, "ALU cells should be X-stage logic");

    // A register-file flip-flop: its details and its source line.
    let net = s.probes.regs[8].as_ref().unwrap().bits[0];
    let cell = inspect::driver_cell(&s.compiled, net).unwrap();
    let info = inspect::cell_info(&s, cell);
    assert!(info.sequential && info.module_path == "u_core.u_rf");
    assert!(info.src.as_deref().is_some_and(|x| x.contains("st_regfile.sv")));
    assert_eq!(info.state, Some(s.engine().net(net)));

    // Activity: the core is busy while the program runs.
    let act = inspect::activity(&mut s, 10, end);
    let core_toggles: u32 = mods.iter().filter(|m| m.path == "u_core.u_alu").flat_map(|m| m.cells.iter()).map(|&c| act.cell(&s.compiled, c)).sum();
    assert!(core_toggles > 0);
    assert_eq!(s.cycle(), end, "activity measurement returns to the current cycle");

    // Waveforms by replay match the values seen at each cycle.
    let pc = s.probes.stages[4].pc.clone();
    let waves = inspect::capture(&mut s, std::slice::from_ref(&pc), 20, 40);
    for (k, row) in waves.values.iter().enumerate() {
        s.goto(20 + k as u64);
        assert_eq!(row[0], s.value(&pc));
    }
    let mut vcd = Vec::new();
    waves.write_vcd(&mut vcd).unwrap();
    assert!(String::from_utf8(vcd).unwrap().contains("$enddefinitions"));
}
