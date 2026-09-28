//! Loads the real sky130 library and the synthesized Strata netlist, when
//! present (run `python3 scripts/synth_gate.py` first).

use std::path::PathBuf;

use strata_netlist::{CellLibrary, Netlist, PortDir};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn liberty() -> PathBuf {
    let pdk = std::env::var("PDK_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join("tools/pdk"));
    pdk.join("sky130A/libs.ref/sky130_fd_sc_hd/lib/sky130_fd_sc_hd__tt_025C_1v80.lib")
}

#[test]
fn sky130_cells_used_by_the_netlist_are_simulatable() {
    let (lib_path, net_path) = (liberty(), repo().join("build/gate/strata_soc.json"));
    if !lib_path.exists() || !net_path.exists() {
        eprintln!("skipping: {} or {} missing", lib_path.display(), net_path.display());
        return;
    }
    let t = std::time::Instant::now();
    let lib = CellLibrary::load(&lib_path).unwrap();
    eprintln!("liberty: {} cells ({} unsupported) in {:?}", lib.cells.len(), lib.unsupported.len(), t.elapsed());
    assert!(lib.cells.len() > 300);
    assert!(lib.cells["sky130_fd_sc_hd__dfxtp_1"].is_sequential());

    let t = std::time::Instant::now();
    let n = Netlist::load(&net_path, "strata_soc").unwrap();
    eprintln!("netlist: {} cells, {} nets, {} instances in {:?}", n.cells.len(), n.nets.len(), n.instances.len(), t.elapsed());
    assert_eq!(n.undef_bits, 0);
    for c in &n.cells {
        let def = lib.cells.get(&c.ty).unwrap_or_else(|| panic!("cell type {} not simulatable", c.ty));
        for p in &c.pins {
            match p.dir {
                PortDir::Input => assert!(def.input_index(&p.name).is_some(), "{}: {}", c.ty, p.name),
                PortDir::Output => assert!(def.output_index(&p.name).is_some(), "{}: {}", c.ty, p.name),
            }
        }
    }
    assert!(n.net_by_name("u_core.x_pc[0]").is_some());
    assert!(n.net_by_name("u_core.u_rf.gen_x[5].q[31]").is_some());
    assert_eq!(n.port("clk").unwrap().dir, PortDir::Input);
}
