//! strata-dbg: the Strata debugger.
//!
//! A native application over a gate-level debug session (strata-debug): the
//! synthesized sky130 netlist runs the machine, and every view (source,
//! instructions, pipeline, bus, modules, cells, waveforms, the 3D scene) is
//! read from that netlist and the debug information of the programs.

mod app;
mod render3d;
mod scene;
mod scene_view;
mod view;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use strata_debug::inspect::{self, StageMap};
use strata_debug::{Session, SessionConfig, SessionFile};
use strata_gate::Compiled;

/// What is selected across all views.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub cell: Option<u32>,
    pub net: Option<u32>,
    pub module: Option<u32>,
    pub pc: Option<u32>,
}

impl Selection {
    pub fn select_cell(&mut self, c: &Compiled, cell: u32) {
        self.cell = Some(cell);
        self.module = Some(c.netlist.cells[cell as usize].instance);
        let out = c.netlist.cells[cell as usize].pins.iter().find(|p| p.dir == strata_netlist::PortDir::Output);
        self.net = out.map(|p| p.net);
    }
}

const USAGE: &str = "usage: strata-dbg [options]
  --netlist <json>     gate-level netlist (default build/gate/strata_soc.json)
  --liberty <lib>      cell library (default: $PDK_ROOT/sky130A/.../tt_025C_1v80.lib)
  --rom <hex>          boot ROM (default build/sw/boot/boot.hex)
  --flash <bin>        boot flash contents (default build/sw/flash.bin)
  --ram <hex>          RAM preload instead of the flash (for ISA tests)
  --sw <dir>           software build directory for debug info (default build/sw)
  --sdb <name=path>    additional debug information
  --session <json>     reopen a saved session (replays its timeline)
  --random-power-on <seed>  pseudo-random initial flip-flop contents

Headless screenshots (no window):
  --screenshot <png>   render the interface offscreen and save it
  --size <WxH>         image size in points (default 1600x1000)
  --goto <cycle>       move the session to this cycle first
  --break <symbol>     run to this function (commit breakpoint) first
  --select <cell>      select a netlist cell by name
  --type <text>        type on the terminal at the start (\\n for newline)
  --color <mode>       3D coloring: stage, module, activity, value";

fn default_liberty() -> PathBuf {
    let root = std::env::var("PDK_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("tools/pdk"));
    root.join("sky130A/libs.ref/sky130_fd_sc_hd/lib/sky130_fd_sc_hd__tt_025C_1v80.lib")
}

#[derive(Default)]
struct Shot {
    path: Option<PathBuf>,
    size: (f32, f32),
    goto: Option<u64>,
    brk: Option<String>,
    select: Option<String>,
    typed: Option<String>,
    color: Option<String>,
}

fn parse_args() -> Result<(SessionConfig, Option<PathBuf>, Shot), String> {
    let mut cfg = SessionConfig {
        netlist: "build/gate/strata_soc.json".into(),
        liberty: default_liberty(),
        rom: "build/sw/boot/boot.hex".into(),
        flash: Some("build/sw/flash.bin".into()),
        ram: None,
        debug: Vec::new(),
        source_root: "sw".into(),
        power_on_seed: None,
        checkpoint_interval: 100_000,
    };
    let mut sw = PathBuf::from("build/sw");
    let mut session = None;
    let mut shot = Shot { size: (1600.0, 1000.0), ..Default::default() };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--netlist" => cfg.netlist = val()?.into(),
            "--liberty" => cfg.liberty = val()?.into(),
            "--rom" => cfg.rom = val()?.into(),
            "--flash" => cfg.flash = Some(val()?.into()),
            "--ram" => {
                cfg.ram = Some(val()?.into());
                cfg.flash = None;
            }
            "--sw" => sw = val()?.into(),
            "--sdb" => {
                let v = val()?;
                let (n, p) = v.split_once('=').ok_or("--sdb wants name=path")?;
                cfg.debug.push((n.to_string(), p.into()));
            }
            "--session" => session = Some(val()?.into()),
            "--random-power-on" => cfg.power_on_seed = Some(val()?.parse().map_err(|_| "bad seed")?),
            "--screenshot" => shot.path = Some(val()?.into()),
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').ok_or("--size wants WxH")?;
                shot.size = (w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            "--goto" => shot.goto = Some(val()?.parse().map_err(|_| "bad cycle")?),
            "--break" => shot.brk = Some(val()?),
            "--select" => shot.select = Some(val()?),
            "--type" => shot.typed = Some(val()?.replace("\\n", "\n")),
            "--color" => shot.color = Some(val()?),
            "-h" | "--help" => return Err(USAGE.into()),
            o => return Err(format!("unknown option {o}\n{USAGE}")),
        }
    }
    // Debug information from the software build: boot ROM, kernel, programs.
    for (name, path) in [("boot", sw.join("boot/boot.sdb")), ("kernel", sw.join("kernel/kernel.sdb"))] {
        if path.exists() {
            cfg.debug.push((name.to_string(), path));
        }
    }
    if let Ok(rd) = std::fs::read_dir(sw.join("user")) {
        let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            let name = d.file_name().unwrap().to_string_lossy().into_owned();
            let sdb = d.join(format!("{name}.sdb"));
            if sdb.exists() {
                cfg.debug.push((name, sdb));
            }
        }
    }
    Ok((cfg, session, shot))
}

fn main() {
    let (cfg, session_file, shot) = match parse_args() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    eprintln!("strata-dbg: loading {}", cfg.netlist.display());
    let compiled = match strata_gate::load(&cfg.netlist, &cfg.liberty, "strata_soc") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("strata-dbg: {e}");
            std::process::exit(1);
        }
    };
    let session = match &session_file {
        Some(p) => std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<SessionFile>(&t).map_err(|e| e.to_string()))
            .and_then(|f| Session::from_file(f, Some(Arc::clone(&compiled)))),
        None => Session::with_compiled(cfg, Arc::clone(&compiled)),
    };
    let mut session = match session {
        Ok(s) => s,
        Err(e) => {
            eprintln!("strata-dbg: {e}");
            std::process::exit(1);
        }
    };
    session.machine.engine.enable_toggle_counts(true);
    if !session.probes.missing.is_empty() {
        eprintln!("strata-dbg: signals not found in the netlist: {:?}", session.probes.missing);
    }
    let mods = inspect::modules(&compiled);
    let stages = StageMap::build(&compiled);
    let mut source_files: Vec<PathBuf> = session.debug.images.iter().flat_map(|i| i.source_files()).collect();
    source_files.sort();
    source_files.dedup();
    let mut scene = scene_view::SceneView::new(&compiled, &mods, &stages);
    let statics = Arc::new(app::Statics { compiled, mods, stages, source_files });

    if let Some(path) = shot.path.clone() {
        screenshot(session, statics, scene, &shot, &path);
        return;
    }
    let _ = &mut scene;

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_inner_size([1600.0, 1000.0]).with_title("Strata debugger"),
        ..Default::default()
    };
    let result = eframe::run_native(
        "strata-dbg",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            let ctx = cc.egui_ctx.clone();
            let worker = worker::Worker::start(session, Box::new(move || ctx.request_repaint()));
            Ok(Box::new(Main { app: app::App::new(worker, statics, scene) }))
        }),
    );
    if let Err(e) = result {
        eprintln!("strata-dbg: {e}");
        std::process::exit(1);
    }
}

struct Main {
    app: app::App,
}

/// Positions the session as requested, renders the interface offscreen with
/// wgpu, and writes a PNG.
fn screenshot(mut session: Session, statics: Arc<app::Statics>, mut scene: scene_view::SceneView, shot: &Shot, path: &std::path::Path) {
    if let Some(t) = &shot.typed {
        session.record(strata_debug::TimelineEvent::Input(strata_debug::InputEvent::Uart(t.clone().into_bytes())));
    }
    if let Some(sym) = &shot.brk {
        let addr = session.debug.images.iter().find_map(|i| i.symbol(sym).map(|s| s.addr));
        match addr {
            Some(a) => {
                let id = session.add_trigger(strata_debug::TriggerKind::Commit(a), sym.clone());
                let r = session.run(200_000_000, &mut || false);
                eprintln!("strata-dbg: {r:?} at cycle {}", session.cycle());
                session.remove_trigger(id);
            }
            None => eprintln!("strata-dbg: no symbol {sym}"),
        }
    }
    if let Some(c) = shot.goto {
        session.goto(c);
    }
    scene.color_by = match shot.color.as_deref() {
        Some("module") => scene_view::ColorBy::Module,
        Some("activity") => scene_view::ColorBy::Activity,
        Some("value") => scene_view::ColorBy::Value,
        _ => scene_view::ColorBy::Stage,
    };
    let select = shot.select.as_ref().and_then(|n| inspect::cell_index(&statics.compiled).get(n.as_str()).copied());
    let size = egui::vec2(shot.size.0, shot.size.1);
    let st = Arc::clone(&statics);
    let mut harness = egui_kittest::Harness::builder().with_size(size).wgpu().build_eframe(move |cc| {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let worker = worker::Worker::start(session, Box::new(|| {}));
        let mut app = app::App::new(worker, st, scene);
        if let Some(c) = select {
            app.sel.select_cell(&statics.compiled, c);
        }
        Main { app }
    });
    // Let activity accumulate and layout settle.
    harness.run_steps(6);
    match harness.render() {
        Ok(img) => match img.save(path) {
            Ok(()) => eprintln!("strata-dbg: wrote {}", path.display()),
            Err(e) => eprintln!("strata-dbg: {e}"),
        },
        Err(e) => eprintln!("strata-dbg: render failed: {e}"),
    }
}

impl eframe::App for Main {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let rs = frame.wgpu_render_state().cloned();
        self.app.frame(ui, rs.as_ref());
    }
}
