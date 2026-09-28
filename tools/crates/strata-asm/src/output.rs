//! Output formats of a linked [`Program`]: ELF executable, flat binary,
//! `$readmemh` hex, listing, and map.

use crate::diag::FileTable;
use crate::link::Program;
use crate::object::{Bind, SymKind};
use std::fmt::Write as _;

/// Largest flat image written by [`flat_binary`] (guards against layouts with
/// sections far apart).
pub const MAX_FLAT_SIZE: u64 = 256 << 20;

/// All non-nobits bytes from the lowest to the highest loaded address, gaps
/// filled with zeros. Returns the base address and the bytes.
pub fn flat_binary(p: &Program) -> Result<(u32, Vec<u8>), String> {
    let loaded: Vec<_> = p
        .sections
        .iter()
        .filter(|s| !s.nobits && s.size > 0)
        .collect();
    let Some(lo) = loaded.iter().map(|s| u64::from(s.addr)).min() else {
        return Ok((0, Vec::new()));
    };
    let hi = loaded
        .iter()
        .map(|s| u64::from(s.addr) + u64::from(s.size))
        .max()
        .unwrap_or(lo);
    if hi - lo > MAX_FLAT_SIZE {
        return Err(format!(
            "flat image would be {} bytes (sections span {lo:#x}..{hi:#x})",
            hi - lo
        ));
    }
    let mut out = vec![0u8; (hi - lo) as usize];
    for s in loaded {
        let at = (u64::from(s.addr) - lo) as usize;
        out[at..at + s.data.len()].copy_from_slice(&s.data);
    }
    Ok((lo as u32, out))
}

/// One little-endian 32-bit word per line in hex (`$readmemh` format),
/// padded with zeros to a whole word.
pub fn hex_words(bin: &[u8]) -> String {
    let mut out = String::with_capacity(bin.len() / 4 * 9 + 9);
    for chunk in bin.chunks(4) {
        let mut w = [0u8; 4];
        w[..chunk.len()].copy_from_slice(chunk);
        let _ = writeln!(out, "{:08x}", u32::from_le_bytes(w));
    }
    out
}

// ----- ELF ------------------------------------------------------------------------------

const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_NOBITS: u32 = 8;
const SHF_WRITE: u32 = 1;
const SHF_ALLOC: u32 = 2;
const SHF_EXECINSTR: u32 = 4;
const SHF_MERGE: u32 = 0x10;
const SHF_STRINGS: u32 = 0x20;
const SHN_ABS: u16 = 0xFFF1;
const EM_RISCV: u16 = 243;

struct StrTab(Vec<u8>);

impl StrTab {
    fn new() -> Self {
        StrTab(vec![0])
    }

    fn add(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        let at = self.0.len() as u32;
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        at
    }
}

fn put16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn pad_to(b: &mut Vec<u8>, align: usize) {
    while b.len() % align != 0 {
        b.push(0);
    }
}

/// Alignment used for a section's `PT_LOAD` segment.
fn seg_align(align: u64) -> u64 {
    align.clamp(1, 4096)
}

/// An ELF32 little-endian RISC-V executable with one `PT_LOAD` segment per
/// non-empty output section, section headers, `.symtab` and `.strtab`.
pub fn write_elf(p: &Program) -> Vec<u8> {
    const EHSIZE: usize = 52;
    const PHENTSIZE: usize = 32;
    const SHENTSIZE: usize = 40;
    let secs: Vec<(usize, &crate::link::OutSection)> = p
        .sections
        .iter()
        .enumerate()
        .filter(|(_, s)| s.size > 0)
        .collect();
    // ELF section index of each program section (0 = omitted).
    let mut shndx = vec![0u16; p.sections.len()];
    for (k, (i, _)) in secs.iter().enumerate() {
        shndx[*i] = (k + 1) as u16;
    }
    let phnum = secs.len();
    let mut out = vec![0u8; EHSIZE + PHENTSIZE * phnum];

    // Section contents.
    let mut offsets = Vec::new();
    for (_, s) in &secs {
        // File offset congruent to the address modulo the segment alignment.
        let a = seg_align(s.align);
        while out.len() as u64 % a != u64::from(s.addr) % a {
            out.push(0);
        }
        offsets.push(out.len() as u32);
        if !s.nobits {
            out.extend_from_slice(&s.data);
        }
    }

    // Symbol table: locals first.
    let mut strtab = StrTab::new();
    let mut symtab = vec![0u8; 16];
    let mut ordered: Vec<_> = p.symbols.iter().collect();
    ordered.sort_by_key(|s| s.bind != Bind::Local);
    let first_global = 1 + ordered.iter().filter(|s| s.bind == Bind::Local).count();
    for s in ordered {
        put32(&mut symtab, strtab.add(&s.name));
        put32(&mut symtab, s.addr);
        put32(&mut symtab, s.size);
        let bind: u8 = match s.bind {
            Bind::Local => 0,
            Bind::Global => 1,
            Bind::Weak => 2,
        };
        let ty: u8 = match s.kind {
            SymKind::NoType => 0,
            SymKind::Object => 1,
            SymKind::Func => 2,
            SymKind::Section => 3,
        };
        symtab.push(bind << 4 | ty);
        symtab.push(0);
        let idx = s
            .section
            .map(|i| shndx[i])
            .filter(|&x| x != 0)
            .unwrap_or(SHN_ABS);
        put16(&mut symtab, idx);
    }
    pad_to(&mut out, 4);
    let symtab_off = out.len() as u32;
    out.extend_from_slice(&symtab);
    let strtab_off = out.len() as u32;
    out.extend_from_slice(&strtab.0);

    let mut shstr = StrTab::new();
    let names: Vec<u32> = secs.iter().map(|(_, s)| shstr.add(&s.name)).collect();
    let symtab_name = shstr.add(".symtab");
    let strtab_name = shstr.add(".strtab");
    let shstrtab_name = shstr.add(".shstrtab");
    let shstrtab_off = out.len() as u32;
    out.extend_from_slice(&shstr.0);
    pad_to(&mut out, 4);
    let shoff = out.len();

    // Section headers.
    let sh = |out: &mut Vec<u8>, f: [u32; 10]| {
        for v in f {
            put32(out, v);
        }
    };
    sh(&mut out, [0; 10]);
    for (k, (_, s)) in secs.iter().enumerate() {
        let mut flags = SHF_ALLOC;
        if s.flags.write {
            flags |= SHF_WRITE;
        }
        if s.flags.exec {
            flags |= SHF_EXECINSTR;
        }
        if s.flags.merge {
            flags |= SHF_MERGE;
        }
        if s.flags.strings {
            flags |= SHF_STRINGS;
        }
        let ty = if s.nobits { SHT_NOBITS } else { SHT_PROGBITS };
        sh(
            &mut out,
            [
                names[k],
                ty,
                flags,
                s.addr,
                offsets[k],
                s.size,
                0,
                0,
                s.align as u32,
                0,
            ],
        );
    }
    let nsec = secs.len() as u32;
    sh(
        &mut out,
        [
            symtab_name,
            SHT_SYMTAB,
            0,
            0,
            symtab_off,
            symtab.len() as u32,
            nsec + 2,
            first_global as u32,
            4,
            16,
        ],
    );
    sh(
        &mut out,
        [
            strtab_name,
            SHT_STRTAB,
            0,
            0,
            strtab_off,
            strtab.0.len() as u32,
            0,
            0,
            1,
            0,
        ],
    );
    sh(
        &mut out,
        [
            shstrtab_name,
            SHT_STRTAB,
            0,
            0,
            shstrtab_off,
            shstr.0.len() as u32,
            0,
            0,
            1,
            0,
        ],
    );
    let shnum = nsec as u16 + 4;

    // ELF header.
    let mut h = Vec::with_capacity(EHSIZE);
    h.extend_from_slice(&[0x7F, b'E', b'L', b'F', 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    put16(&mut h, 2); // ET_EXEC
    put16(&mut h, EM_RISCV);
    put32(&mut h, 1);
    put32(&mut h, p.entry);
    put32(&mut h, if phnum > 0 { EHSIZE as u32 } else { 0 });
    put32(&mut h, shoff as u32);
    put32(&mut h, 0); // e_flags: soft-float ABI, no RVC
    put16(&mut h, EHSIZE as u16);
    put16(&mut h, PHENTSIZE as u16);
    put16(&mut h, phnum as u16);
    put16(&mut h, SHENTSIZE as u16);
    put16(&mut h, shnum);
    put16(&mut h, shnum - 1);
    out[..EHSIZE].copy_from_slice(&h);

    // Program headers.
    let mut ph = Vec::with_capacity(PHENTSIZE * phnum);
    for (k, (_, s)) in secs.iter().enumerate() {
        put32(&mut ph, 1); // PT_LOAD
        put32(&mut ph, offsets[k]);
        put32(&mut ph, s.addr);
        put32(&mut ph, s.addr);
        put32(&mut ph, if s.nobits { 0 } else { s.size });
        put32(&mut ph, s.size);
        let mut pf = 4;
        if s.flags.write {
            pf |= 2;
        }
        if s.flags.exec {
            pf |= 1;
        }
        put32(&mut ph, pf);
        put32(&mut ph, seg_align(s.align) as u32);
    }
    out[EHSIZE..EHSIZE + ph.len()].copy_from_slice(&ph);
    out
}

// ----- listing and map ------------------------------------------------------------------------

/// Listing: per input file, one line per statement with address, encoding
/// (words for instructions, bytes for data) and source text.
pub fn listing(p: &Program, files: &FileTable) -> String {
    const MAX_BYTES: usize = 64;
    let mut out = String::new();
    for (inp, lines) in p.inputs.iter().zip(&p.listing) {
        let _ = writeln!(out, "# {}", inp.path);
        let mut cur_file = inp.file;
        for l in lines {
            if l.loc.file != cur_file {
                cur_file = l.loc.file;
                let _ = writeln!(out, "# {}", files.name(cur_file));
            }
            let addr = l.addr.map_or_else(|| " ".repeat(8), |a| format!("{a:08x}"));
            let chunks: Vec<String> = if l.code {
                l.bytes
                    .chunks(4)
                    .map(|c| {
                        let mut w = [0u8; 4];
                        w[..c.len()].copy_from_slice(c);
                        format!("{:08x}", u32::from_le_bytes(w))
                    })
                    .collect()
            } else {
                l.bytes[..l.bytes.len().min(MAX_BYTES)]
                    .chunks(8)
                    .map(|c| {
                        c.iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect()
            };
            let first = chunks.first().cloned().unwrap_or_default();
            let _ = writeln!(out, "{:>6}  {addr}  {first:<23}  {}", l.loc.line, l.text);
            let mut a = l.addr.unwrap_or(0);
            let step = if l.code { 4 } else { 8 };
            for c in chunks.iter().skip(1) {
                a = a.wrapping_add(step);
                let _ = writeln!(out, "{:>6}  {a:08x}  {c}", "");
            }
            if !l.code && l.bytes.len() > MAX_BYTES {
                let _ = writeln!(out, "{:>6}  {:8}  ... ({} bytes)", "", "", l.bytes.len());
            }
        }
        out.push('\n');
    }
    out
}

/// Map: memory regions, output sections with their input sections, and all
/// symbols by address.
pub fn map(p: &Program) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Entry point: 0x{:08x}\n", p.entry);
    let _ = writeln!(out, "Memory regions:");
    let _ = writeln!(
        out,
        "  {:<12} {:>10} {:>10} {:>10}",
        "name", "origin", "length", "used"
    );
    for r in &p.regions {
        let pct = if r.length > 0 {
            r.used as f64 * 100.0 / r.length as f64
        } else {
            0.0
        };
        let _ = writeln!(
            out,
            "  {:<12} 0x{:08x} 0x{:08x} 0x{:08x} ({pct:.1}%)",
            r.name, r.origin, r.length, r.used
        );
    }
    let _ = writeln!(out, "\nOutput sections:");
    for (i, s) in p.sections.iter().enumerate() {
        let _ = writeln!(
            out,
            "{:<16} 0x{:08x} 0x{:08x}  align {:<4} {:<3} {}{}",
            s.name,
            s.addr,
            s.size,
            s.align,
            s.flags.text(),
            s.region,
            if s.nobits { " nobits" } else { "" }
        );
        for inp in &s.inputs {
            let path = p
                .inputs
                .get(inp.obj as usize)
                .map_or("COMMON", |f| f.path.as_str());
            let _ = writeln!(
                out,
                "    0x{:08x} 0x{:08x}  align {:<4} {path}({})",
                inp.addr, inp.size, inp.align, inp.name
            );
        }
        for sym in p.symbols.iter().filter(|y| y.section == Some(i)) {
            let _ = writeln!(out, "        0x{:08x}  {}", sym.addr, sym.name);
        }
    }
    let _ = writeln!(out, "\nSymbols:");
    for s in &p.symbols {
        let sec = s.section.map_or("*ABS*", |i| p.sections[i].name.as_str());
        let file = s
            .file
            .and_then(|f| p.inputs.get(f))
            .map_or("", |f| f.path.as_str());
        let _ = writeln!(
            out,
            "  0x{:08x} 0x{:08x} {:<6} {:<6} {:<10} {:<24} {file}",
            s.addr,
            s.size,
            s.bind.as_str(),
            s.kind.as_str(),
            sec,
            s.name
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_pads_last_word() {
        assert_eq!(hex_words(&[0x13, 0, 0, 0, 0xAA]), "00000013\n000000aa\n");
        assert_eq!(hex_words(&[]), "");
    }
}
