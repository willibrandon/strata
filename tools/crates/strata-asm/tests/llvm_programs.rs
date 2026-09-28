//! Differential test (b): whole C programs.
//!
//! The C files in `tests/c/` are compiled with exactly the flags of
//! `docs/toolchain.md` (`clang -S`), then linked twice:
//!
//! * strata-as (the `strata-as` binary) with `--org ADDR`;
//! * `llvm-mc -filetype=obj` + `ld.lld` with a linker script equivalent to
//!   `--org`: output sections `.text`, `.rodata`, `.data`, `.bss` in that
//!   order, each taking `NAME` then `NAME.*` input sections (plus the
//!   small-data variants), in command-line order.
//!
//! The loaded bytes, section placement, symbol addresses and disassembly
//! must be identical. `ld.lld` runs with `-O0` so that it does not merge
//! duplicate strings of `SHF_MERGE` sections (strata-as places input
//! sections verbatim; merging is an optimization not in the spec).
//!
//! The `.sdb` line table is also compared with the DWARF line table that
//! llvm-mc builds from the same `.loc` directives (`llvm-dwarfdump
//! --debug-line` of the lld output): every row's address, line, column,
//! file and `is_stmt`, and the end of every sequence (which `.sdb` marks
//! with a `line == 0` row).
//!
//! A second test compiles with `-ffunction-sections -fdata-sections` and
//! links with `--gc-sections` against `ld.lld --gc-sections`, using layouts
//! with `KEEP()` roots and, in one configuration, `--keep-symbol` (lld's
//! `--undefined`). Besides bytes, symbols, disassembly and line rows (of
//! live code; lld keeps rows of removed functions at tombstone addresses),
//! the set of removed sections must equal lld's `--print-gc-sections`.
//! With `-O0`, lld keeps `SHF_MERGE` string sections whole, as strata-as
//! does; with merging enabled lld would also drop individual dead strings.

mod common;

use common::{run, run_status, scratch, tools};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use strata_asm::sdb::DebugInfo;

/// Compilation flags from docs/toolchain.md.
const CLANG_FLAGS: &[&str] = &[
    "--target=riscv32-unknown-elf",
    "-march=rv32i_zicsr_zifencei",
    "-mabi=ilp32",
    "-mno-relax",
    "-mcmodel=medlow",
    "-fno-pic",
    "-ffreestanding",
    "-nostdlib",
    "-fno-asynchronous-unwind-tables",
    "-fno-unwind-tables",
    "-O2",
    "-gline-tables-only",
    "-S",
];

const LINKER_SCRIPT: &str = "
SECTIONS {
  . = ORIGIN_ADDR;
  .text   : { *(.text) *(.text.*) }
  .rodata : { *(.rodata) *(.rodata.*) *(.srodata) *(.srodata.*) }
  .data   : { *(.data) *(.data.*) *(.sdata) *(.sdata.*) }
  .bss    : { *(.bss) *(.bss.*) *(.sbss) *(.sbss.*) *(COMMON) }
  /DISCARD/ : { *(.riscv.attributes) *(.comment) *(.note*) *(.llvm_addrsig) }
}
";

fn c_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/c")
}

/// Compiles `name.c` into `dir/name.s`, adding `extra` flags.
fn compile(clang: &Path, dir: &Path, name: &str, extra: &[&str]) -> PathBuf {
    let out = dir.join(format!("{name}.s"));
    run(Command::new(clang)
        .args(CLANG_FLAGS)
        .args(extra)
        .arg(c_dir().join(format!("{name}.c")))
        .arg("-o")
        .arg(&out));
    out
}

/// Instruction lines of `llvm-objdump -d`, without symbol annotations.
fn disassembly(objdump: &Path, elf: &Path) -> Vec<String> {
    let text = run(Command::new(objdump)
        .args(["-d", "--mattr=+zicsr,+zifencei", "--no-leading-headers"])
        .arg(elf));
    text.lines()
        .filter(|l| {
            l.trim_start()
                .split(':')
                .next()
                .is_some_and(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_hexdigit()))
                && l.contains('\t')
        })
        .map(|l| {
            let l = match l.find(" <") {
                Some(i) if l[i..].contains('>') => {
                    format!("{}{}", &l[..i], &l[i + l[i..].find('>').unwrap_or(0) + 1..])
                }
                _ => l.to_string(),
            };
            l.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .collect()
}

/// `(address, name)` of every symbol except mapping and assembler-local ones.
fn symbols(nm: &Path, elf: &Path) -> BTreeSet<(u64, String)> {
    run(Command::new(nm).args(["-n", "--defined-only"]).arg(elf))
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let addr = u64::from_str_radix(parts.next()?, 16).ok()?;
            let _kind = parts.next()?;
            let name = parts.next()?.to_string();
            (!name.starts_with('$') && !name.starts_with(".L")).then_some((addr, name))
        })
        .collect()
}

/// `(name, address, size)` of the loadable sections.
fn sections(readelf: &Path, elf: &Path) -> Vec<(String, u64, u64)> {
    run(Command::new(readelf).arg("-SW").arg(elf))
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l.strip_prefix('[')?.split_once(']')?.1;
            let f: Vec<&str> = rest.split_whitespace().collect();
            let name = (*f.first()?).to_string();
            if ![".text", ".rodata", ".data", ".bss"].contains(&name.as_str()) {
                return None;
            }
            Some((
                name,
                u64::from_str_radix(f[2], 16).ok()?,
                u64::from_str_radix(f[4], 16).ok()?,
            ))
        })
        .collect()
}

/// A line-table row: address, line, column, file base name, is_stmt.
type Row = (u32, u32, u32, String, bool);

fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Rows of `llvm-dwarfdump --debug-line`, with end-of-sequence rows as
/// `line == 0` rows unless a real row starts at the same address.
fn dwarf_rows(dwarfdump: &Path, elf: &Path) -> Vec<Row> {
    let text = run(Command::new(dwarfdump).arg("--debug-line").arg(elf));
    let mut files: Vec<String> = Vec::new();
    let mut pending_name = false;
    let mut rows = Vec::new();
    let mut ends = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("debug_line[") {
            files.clear();
        } else if t.starts_with("file_names[") {
            pending_name = true;
        } else if let Some(name) = t.strip_prefix("name: ").filter(|_| pending_name) {
            files.push(base_name(name.trim_matches('"')));
            pending_name = false;
        } else if t.starts_with("0x") {
            let f: Vec<&str> = t.split_whitespace().collect();
            let addr = u32::from_str_radix(f[0].trim_start_matches("0x"), 16).unwrap_or(0);
            if t.contains("end_sequence") {
                ends.push(addr);
                continue;
            }
            let num = |i: usize| f[i].parse::<u32>().unwrap_or(0);
            let file = files.get(num(3) as usize).cloned().unwrap_or_default();
            rows.push((addr, num(1), num(2), file, t.contains("is_stmt")));
        }
    }
    let starts: BTreeSet<u32> = rows.iter().map(|r| r.0).collect();
    let end_file = |a: u32| {
        rows.iter()
            .filter(|r| r.0 < a)
            .max_by_key(|r| r.0)
            .map(|r| r.3.clone())
            .unwrap_or_default()
    };
    let mut all: Vec<Row> = ends
        .iter()
        .filter(|a| !starts.contains(a))
        .map(|&a| (a, 0, 0, end_file(a), false))
        .collect();
    all.extend(rows);
    all.sort();
    all.dedup();
    all
}

struct Tools {
    clang: PathBuf,
    mc: PathBuf,
    lld: PathBuf,
    objcopy: PathBuf,
    objdump: PathBuf,
    nm: PathBuf,
    readelf: PathBuf,
    dwarfdump: PathBuf,
}

/// Links `units` (C file stems) plus the runtime both ways at `org` and
/// compares the results. Returns the numbers of instructions and of line
/// table rows compared.
fn check_program(t: &Tools, name: &str, units: &[&str], org: u32) -> (usize, usize) {
    let dir = scratch(&format!("llvm_programs_{name}_{org:x}"));
    let mut asm_files: Vec<PathBuf> = units
        .iter()
        .map(|u| compile(&t.clang, &dir, u, &[]))
        .collect();
    asm_files.push(c_dir().join("runtime.s"));

    // strata-as, through the command-line binary.
    let s_elf = dir.join("strata.elf");
    let s_bin = dir.join("strata.bin");
    let s_sdb = dir.join("strata.sdb");
    let (ok, _, stderr) = run_status(
        Command::new(env!("CARGO_BIN_EXE_strata-as"))
            .arg("--org")
            .arg(format!("{org:#x}"))
            .arg("-o")
            .arg(&s_elf)
            .arg("--bin")
            .arg(&s_bin)
            .arg("--hex")
            .arg(dir.join("strata.hex"))
            .arg("--debug")
            .arg(&s_sdb)
            .arg("--listing")
            .arg(dir.join("strata.lst"))
            .arg("--map")
            .arg(dir.join("strata.map"))
            .args(&asm_files),
    );
    assert!(ok, "strata-as failed for {name}:\n{stderr}");
    assert!(stderr.is_empty(), "unexpected diagnostics:\n{stderr}");

    // llvm-mc + ld.lld.
    let mut objs = Vec::new();
    for (i, s) in asm_files.iter().enumerate() {
        let o = dir.join(format!("{i}.o"));
        run(Command::new(&t.mc)
            .args(common::MC_ARGS)
            .arg("-filetype=obj")
            .arg(s)
            .arg("-o")
            .arg(&o));
        objs.push(o);
    }
    let script = dir.join("org.ld");
    std::fs::write(
        &script,
        LINKER_SCRIPT.replace("ORIGIN_ADDR", &format!("{org:#x}")),
    )
    .unwrap();
    let l_elf = dir.join("lld.elf");
    let l_bin = dir.join("lld.bin");
    run(Command::new(&t.lld)
        .args(["-O0", "--image-base=0", "-e", "_start", "-T"])
        .arg(&script)
        .args(&objs)
        .arg("-o")
        .arg(&l_elf));
    run(Command::new(&t.objcopy)
        .args(["-O", "binary"])
        .arg(&l_elf)
        .arg(&l_bin));

    let s_bytes = std::fs::read(&s_bin).unwrap();
    let l_bytes = std::fs::read(&l_bin).unwrap();
    assert_eq!(
        sections(&t.readelf, &s_elf),
        sections(&t.readelf, &l_elf),
        "{name}: section placement differs"
    );
    if s_bytes != l_bytes {
        let first = s_bytes
            .iter()
            .zip(&l_bytes)
            .position(|(a, b)| a != b)
            .unwrap_or(s_bytes.len().min(l_bytes.len()));
        panic!(
            "{name}: loaded bytes differ at offset {first:#x} (strata {} bytes, lld {} bytes)",
            s_bytes.len(),
            l_bytes.len()
        );
    }
    let (s_syms, l_syms) = (symbols(&t.nm, &s_elf), symbols(&t.nm, &l_elf));
    assert_eq!(s_syms, l_syms, "{name}: symbol tables differ");
    let (s_dis, l_dis) = (
        disassembly(&t.objdump, &s_elf),
        disassembly(&t.objdump, &l_elf),
    );
    assert_eq!(s_dis, l_dis, "{name}: disassembly differs");

    // Hex output is the flat binary as words.
    let hex = std::fs::read_to_string(dir.join("strata.hex")).unwrap();
    assert_eq!(hex.lines().count(), s_bytes.len().div_ceil(4));

    // Debug info: C line rows, per-instruction assembler lines, symbols.
    let info = DebugInfo::from_json(&std::fs::read_to_string(&s_sdb).unwrap()).unwrap();
    assert_eq!(info.entry, info.symbol_addr("_start").unwrap());
    for u in units {
        assert!(
            info.files.iter().any(|f| f.ends_with(&format!("{u}.c"))),
            "{u}.c missing from files: {:?}",
            info.files
        );
        let entry_fn = if *u == "math64" {
            "math_main".to_string()
        } else {
            format!("{u}_main")
        };
        let main = info
            .symbols
            .iter()
            .find(|s| s.name == entry_fn)
            .expect("entry function symbol");
        assert_eq!(
            (
                main.kind.as_str(),
                main.bind.as_str(),
                main.section.as_deref()
            ),
            ("func", "global", Some(".text"))
        );
        assert!(main.size > 0);
    }
    // One asm_lines row per emitted instruction (alignment nops have none).
    let insn_addrs: BTreeSet<u32> = s_dis
        .iter()
        .filter_map(|l| u32::from_str_radix(l.split(':').next()?.trim(), 16).ok())
        .collect();
    let rows: BTreeSet<u32> = info.asm_lines.iter().map(|r| r.0).collect();
    assert_eq!(rows.len(), info.asm_lines.len(), "duplicate asm_lines rows");
    assert!(
        rows.is_subset(&insn_addrs),
        "asm_lines rows at non-instruction addresses"
    );
    assert!(
        insn_addrs.len() - rows.len() <= 4,
        "instructions without asm_lines rows"
    );
    assert!(
        info.lines.windows(2).all(|w| w[0].0 < w[1].0),
        "line rows sorted by address"
    );
    assert!(!info.lines.is_empty());
    let mut ours: Vec<Row> = info
        .lines
        .iter()
        .map(|&(addr, file, line, col, is_stmt)| {
            (
                addr,
                line,
                col,
                base_name(&info.files[file as usize]),
                is_stmt != 0,
            )
        })
        .collect();
    ours.sort();
    let theirs = dwarf_rows(&t.dwarfdump, &l_elf);
    if ours != theirs {
        let first = ours
            .iter()
            .zip(&theirs)
            .position(|(a, b)| a != b)
            .unwrap_or(ours.len().min(theirs.len()));
        panic!(
            "{name}: line tables differ at row {first} ({} vs {} rows): strata {:?} dwarf {:?}",
            ours.len(),
            theirs.len(),
            ours.get(first),
            theirs.get(first)
        );
    }
    let rows = ours.len();
    (s_dis.len(), rows)
}

/// Locates the LLVM tools, or returns `None` (after a note) to skip `test`.
fn find_tools(test: &str) -> Option<Tools> {
    let names = [
        "clang",
        "llvm-mc",
        "ld.lld",
        "llvm-objcopy",
        "llvm-objdump",
        "llvm-nm",
        "llvm-readelf",
        "llvm-dwarfdump",
    ];
    let found = tools(test, &names)?;
    Some(Tools {
        clang: found[0].clone(),
        mc: found[1].clone(),
        lld: found[2].clone(),
        objcopy: found[3].clone(),
        objdump: found[4].clone(),
        nm: found[5].clone(),
        readelf: found[6].clone(),
        dwarfdump: found[7].clone(),
    })
}

#[test]
fn c_programs_match_llvm_mc_and_lld() {
    let Some(t) = find_tools("c_programs_match_llvm_mc_and_lld") else {
        return;
    };
    let (mut insns, mut rows, mut links) = (0, 0, 0);
    let mut check = |name: &str, units: &[&str], org: u32| {
        let (i, r) = check_program(&t, name, units, org);
        insns += i;
        rows += r;
        links += 1;
    };
    // The whole program at the specified origin, and at an origin whose low
    // bits make many %hi/%lo pairs round (negative %lo).
    for org in [0x1000_0000u32, 0x1234_5800] {
        check("all", &["sort", "kernel", "math64"], org);
    }
    for unit in ["sort", "kernel", "math64"] {
        let others: Vec<&str> = ["sort", "kernel", "math64"]
            .into_iter()
            .filter(|u| *u != unit)
            .collect();
        // Each unit first, then the others, to vary placement.
        let order = [&[unit][..], &others[..]].concat();
        check(unit, &order, 0x1000_0000);
    }
    eprintln!("whole-program comparison: {links} links, {insns} instructions and {rows} line-table rows identical");
}

/// strata-as layout for the `--gc-sections` comparison.
const GC_LAYOUT: &str = "
memory RAM origin=0x10000000 length=1M
entry _start
output .text   in RAM { KEEP(.text.start) KEEP(.text.gc_root) .text .text.* }
output .rodata in RAM { .rodata .rodata.* .srodata .srodata.* }
output .data   in RAM { .data .data.* .sdata .sdata.* }
output .bss    in RAM nobits { .bss .bss.* .sbss .sbss.* COMMON }
";

/// The equivalent ld.lld linker script.
const GC_SCRIPT: &str = "
SECTIONS {
  . = 0x10000000;
  .text   : { KEEP(*(.text.start)) KEEP(*(.text.gc_root)) *(.text) *(.text.*) }
  .rodata : { *(.rodata) *(.rodata.*) *(.srodata) *(.srodata.*) }
  .data   : { *(.data) *(.data.*) *(.sdata) *(.sdata.*) }
  .bss    : { *(.bss) *(.bss.*) *(.sbss) *(.sbss.*) *(COMMON) }
  /DISCARD/ : { *(.riscv.attributes) *(.comment) *(.note*) *(.llvm_addrsig) }
}
";

/// File name of a path.
fn file_name(p: &Path) -> String {
    p.file_name()
        .map_or(String::new(), |n| n.to_string_lossy().into_owned())
}

/// `name -> size` of the sections of an object file.
fn object_section_sizes(readelf: &Path, obj: &Path) -> std::collections::HashMap<String, u64> {
    run(Command::new(readelf).arg("-SW").arg(obj))
        .lines()
        .filter_map(|l| {
            let rest = l.trim_start().strip_prefix('[')?.split_once(']')?.1;
            let f: Vec<&str> = rest.split_whitespace().collect();
            Some((
                (*f.first()?).to_string(),
                u64::from_str_radix(f.get(4)?, 16).ok()?,
            ))
        })
        .collect()
}

/// Links the units plus the runtime with `--gc-sections` both ways and
/// compares the results. Returns (instructions, line rows, removed sections).
fn check_gc_program(t: &Tools, name: &str, units: &[&str], keep: &[&str]) -> (usize, usize, usize) {
    let dir = scratch(&format!("llvm_gc_{name}"));
    let mut asm_files: Vec<PathBuf> = units
        .iter()
        .map(|u| {
            compile(
                &t.clang,
                &dir,
                u,
                &["-ffunction-sections", "-fdata-sections"],
            )
        })
        .collect();
    asm_files.push(c_dir().join("runtime.s"));

    let layout = dir.join("gc.ld");
    std::fs::write(&layout, GC_LAYOUT).unwrap();
    let (s_elf, s_bin, s_sdb, s_map) = (
        dir.join("strata.elf"),
        dir.join("strata.bin"),
        dir.join("strata.sdb"),
        dir.join("strata.map"),
    );
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_strata-as"));
    cmd.arg("-T").arg(&layout).arg("--gc-sections");
    for k in keep {
        cmd.arg("--keep-symbol").arg(k);
    }
    cmd.arg("-o")
        .arg(&s_elf)
        .arg("--bin")
        .arg(&s_bin)
        .arg("--debug")
        .arg(&s_sdb)
        .arg("--map")
        .arg(&s_map);
    let (ok, _, stderr) = run_status(cmd.args(&asm_files));
    assert!(
        ok && stderr.is_empty(),
        "strata-as failed for {name}:\n{stderr}"
    );

    let mut objs = Vec::new();
    for (i, s) in asm_files.iter().enumerate() {
        let o = dir.join(format!("{i}.o"));
        run(Command::new(&t.mc)
            .args(common::MC_ARGS)
            .arg("-filetype=obj")
            .arg(s)
            .arg("-o")
            .arg(&o));
        objs.push(o);
    }
    let script = dir.join("gc_lld.ld");
    std::fs::write(&script, GC_SCRIPT).unwrap();
    let (l_elf, l_bin) = (dir.join("lld.elf"), dir.join("lld.bin"));
    let mut cmd = Command::new(&t.lld);
    cmd.args([
        "-O0",
        "--image-base=0",
        "-e",
        "_start",
        "--gc-sections",
        "--print-gc-sections",
        "-T",
    ])
    .arg(&script);
    for k in keep {
        cmd.arg(format!("--undefined={k}"));
    }
    let (ok, out, err) = run_status(cmd.args(&objs).arg("-o").arg(&l_elf));
    assert!(ok, "ld.lld failed:\n{err}");
    run(Command::new(&t.objcopy)
        .args(["-O", "binary"])
        .arg(&l_elf)
        .arg(&l_bin));

    // Same placement, bytes, symbols and code.
    let placement = sections(&t.readelf, &s_elf);
    assert_eq!(
        placement,
        sections(&t.readelf, &l_elf),
        "{name}: section placement differs"
    );
    let (s_bytes, l_bytes) = (
        std::fs::read(&s_bin).unwrap(),
        std::fs::read(&l_bin).unwrap(),
    );
    if s_bytes != l_bytes {
        let first = s_bytes
            .iter()
            .zip(&l_bytes)
            .position(|(a, b)| a != b)
            .unwrap_or(s_bytes.len().min(l_bytes.len()));
        panic!(
            "{name}: loaded bytes differ at offset {first:#x} (strata {} bytes, lld {} bytes)",
            s_bytes.len(),
            l_bytes.len()
        );
    }
    assert_eq!(
        symbols(&t.nm, &s_elf),
        symbols(&t.nm, &l_elf),
        "{name}: symbol tables differ"
    );
    let s_dis = disassembly(&t.objdump, &s_elf);
    assert_eq!(
        s_dis,
        disassembly(&t.objdump, &l_elf),
        "{name}: disassembly differs"
    );

    // Same removed sections: the map's list versus lld's report, ignoring the
    // empty sections lld also reports (clang's `.text` of each unit).
    let ours: BTreeSet<String> = std::fs::read_to_string(&s_map)
        .unwrap()
        .split("Discarded by --gc-sections:")
        .nth(1)
        .unwrap_or("")
        .lines()
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(|entry| entry.rsplit('/').next().unwrap_or(entry).to_string())
        .collect();
    let sizes: Vec<_> = objs
        .iter()
        .map(|o| object_section_sizes(&t.readelf, o))
        .collect();
    let theirs: BTreeSet<String> = format!("{out}{err}")
        .lines()
        .filter_map(|l| l.strip_prefix("removing unused section "))
        .filter_map(|entry| {
            let (obj, sec) = entry.split_once(":(")?;
            let sec = sec.strip_suffix(')')?;
            let index: usize = file_name(Path::new(obj)).strip_suffix(".o")?.parse().ok()?;
            (sizes[index].get(sec).copied().unwrap_or(0) > 0)
                .then(|| format!("{}({sec})", file_name(&asm_files[index])))
        })
        .collect();
    assert_eq!(ours, theirs, "{name}: removed sections differ");

    // Line rows of live code match; lld keeps rows of removed functions at
    // tombstone addresses outside the program, which are ignored.
    let info = DebugInfo::from_json(&std::fs::read_to_string(&s_sdb).unwrap()).unwrap();
    let text = placement
        .iter()
        .find(|s| s.0 == ".text")
        .expect("text section");
    let (lo, hi) = (text.1 as u32, (text.1 + text.2) as u32);
    let mut rows: Vec<Row> = info
        .lines
        .iter()
        .map(|&(addr, file, line, col, is_stmt)| {
            (
                addr,
                line,
                col,
                base_name(&info.files[file as usize]),
                is_stmt != 0,
            )
        })
        .collect();
    rows.sort();
    let dwarf: Vec<Row> = dwarf_rows(&t.dwarfdump, &l_elf)
        .into_iter()
        .filter(|r| (lo..=hi).contains(&r.0))
        .collect();
    assert_eq!(rows, dwarf, "{name}: line tables differ");
    (s_dis.len(), rows.len(), ours.len())
}

#[test]
fn gc_sections_match_lld() {
    let Some(t) = find_tools("gc_sections_match_lld") else {
        return;
    };
    let units = ["sort", "kernel", "math64", "gc"];
    let (i1, r1, d1) = check_gc_program(&t, "roots", &units, &[]);
    let (i2, r2, d2) = check_gc_program(&t, "keep_symbol", &units, &["dead_entry"]);
    // Keeping dead_entry revives it and what it reaches (the cycle, its data).
    assert!(d1 >= 15, "only {d1} sections removed");
    assert!(d2 < d1 && i2 > i1);
    let reordered = ["gc", "math64", "kernel", "sort"];
    let (i3, r3, d3) = check_gc_program(&t, "reordered", &reordered, &[]);
    assert_eq!((i3, d3), (i1, d1));
    eprintln!(
        "gc comparison: 3 links, {} instructions, {} line rows identical; {} + {} + {} removed sections identical to ld.lld",
        i1 + i2 + i3,
        r1 + r2 + r3,
        d1,
        d2,
        d3
    );
}
