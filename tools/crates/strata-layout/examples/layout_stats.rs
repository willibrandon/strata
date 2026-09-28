//! Prints what was read from a layout: counts, wire totals and one net.
//!   layout_stats <result dir> <pdk sky130A dir> [net]
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = Path::new(&a[1]);
    let lib = Path::new(&a[2]).join("libs.ref/sky130_fd_sc_hd");
    let l = strata_layout::Layout::load(
        &dir.join("6_final.def"),
        &[&lib.join("techlef/sky130_fd_sc_hd__nom.tlef"), &lib.join("lef/sky130_fd_sc_hd.lef")],
        Some(&dir.join("6_final.spef")),
    )
    .unwrap();
    let wires: usize = l.nets.iter().map(|n| n.wires.len()).sum();
    let vias: usize = l.nets.iter().map(|n| n.vias.len()).sum();
    let length: f64 = l.nets.iter().map(|n| n.length()).sum();
    let capped = l.nets.iter().filter(|n| n.cap.is_some()).count();
    println!("design {} die {:?} rows {} layers {:?}", l.design, l.die, l.rows.len(), l.layers.iter().map(|x| &x.name).collect::<Vec<_>>());
    println!("components {} pins {} nets {} wires {wires} vias {vias} length {:.0} um, nets with SPEF cap {capped}", l.components.len(), l.pins.len(), l.nets.len(), length);
    if let Some(name) = a.get(3) {
        let n = l.nets.iter().find(|n| &n.name == name).expect("no such net");
        println!("{} pins {:?} cap {:?} length {:.1}", n.name, n.pins, n.cap, n.length());
        for w in n.wires.iter().take(8) {
            println!("  wire {:?}", w);
        }
    }
}
