//! Helpers shared by the integration tests: locating the LLVM tools used
//! for differential testing, scratch directories, running commands, and a
//! deterministic random number generator.

#![allow(dead_code)]

pub mod unit;

use std::path::{Path, PathBuf};
use std::process::Command;
use strata_asm::{assemble_sources, LayoutSpec, Options};

/// Directories searched for LLVM tools, in order. `STRATA_LLVM_BIN` overrides.
fn llvm_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("STRATA_LLVM_BIN") {
        dirs.push(PathBuf::from(d));
    }
    dirs.push(PathBuf::from("/opt/homebrew/opt/llvm/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/opt/lld/bin"));
    dirs.push(PathBuf::from("/usr/local/opt/llvm/bin"));
    dirs.push(PathBuf::from("/usr/local/opt/lld/bin"));
    dirs
}

/// Full path of an LLVM tool, if installed.
pub fn tool(name: &str) -> Option<PathBuf> {
    llvm_dirs()
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Returns the paths of all `names`, or prints a note and returns `None`
/// (the caller then skips the test).
pub fn tools(test: &str, names: &[&str]) -> Option<Vec<PathBuf>> {
    let found: Vec<Option<PathBuf>> = names.iter().map(|n| tool(n)).collect();
    if found.iter().any(Option::is_none) {
        let missing: Vec<&str> = names
            .iter()
            .zip(&found)
            .filter(|(_, f)| f.is_none())
            .map(|(n, _)| *n)
            .collect();
        eprintln!(
            "note: skipping {test}: LLVM tools not found: {} (set STRATA_LLVM_BIN)",
            missing.join(", ")
        );
        return None;
    }
    Some(found.into_iter().map(Option::unwrap_or_default).collect())
}

/// A fresh scratch directory for one test.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Runs a command; panics with its output if it fails. Returns stdout.
pub fn run(cmd: &mut Command) -> String {
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("cannot run {cmd:?}: {e}"));
    if !out.status.success() {
        panic!(
            "command failed: {cmd:?}\n--- stdout\n{}\n--- stderr\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Runs a command and returns (success, stdout, stderr).
pub fn run_status(cmd: &mut Command) -> (bool, String, String) {
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("cannot run {cmd:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Common llvm-mc arguments for the Strata ISA (no relaxation).
pub const MC_ARGS: &[&str] = &["-triple=riscv32", "-mattr=+zicsr,+zifencei,-relax"];

/// Assembles `src` with llvm-mc, links it with ld.lld at `org`, and returns
/// the flat bytes of `.text`.
pub fn llvm_text_bytes(dir: &Path, name: &str, src: &str, org: u32) -> Vec<u8> {
    let [mc, lld, objcopy] =
        [tool("llvm-mc"), tool("ld.lld"), tool("llvm-objcopy")].map(|t| t.expect("tool"));
    let s = dir.join(format!("{name}.s"));
    let o = dir.join(format!("{name}.o"));
    let elf = dir.join(format!("{name}.elf"));
    let bin = dir.join(format!("{name}.bin"));
    std::fs::write(&s, src).unwrap();
    run(Command::new(mc)
        .args(MC_ARGS)
        .arg("-filetype=obj")
        .arg(&s)
        .arg("-o")
        .arg(&o));
    run(Command::new(lld)
        .arg(format!("-Ttext=0x{org:x}"))
        .arg("-e")
        .arg(format!("0x{org:x}"))
        .arg("--image-base=0")
        .arg(&o)
        .arg("-o")
        .arg(&elf));
    run(Command::new(objcopy)
        .args(["-O", "binary", "--only-section=.text"])
        .arg(&elf)
        .arg(&bin));
    std::fs::read(&bin).unwrap()
}

/// Assembles and links sources with strata-as (library), panicking with the
/// rendered diagnostics on error.
pub fn strata(
    sources: &[(&str, &str)],
    layout: LayoutSpec,
) -> (strata_asm::link::Program, strata_asm::diag::FileTable) {
    let opts = Options {
        layout,
        ..Options::default()
    };
    let out = assemble_sources(sources, &opts);
    match out.program {
        Some(p) => (p, out.files),
        None => panic!(
            "strata-as failed:\n{}",
            out.diags.render(&out.files, "strata-as")
        ),
    }
}

/// Bytes of the output section `name`.
pub fn section_bytes(p: &strata_asm::link::Program, name: &str) -> Vec<u8> {
    p.sections
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.data.clone())
        .unwrap_or_default()
}

/// Little-endian words of a byte slice.
pub fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks(4)
        .map(|c| {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            u32::from_le_bytes(w)
        })
        .collect()
}

/// Deterministic xorshift64* generator.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next_u64() % ((hi - lo + 1) as u64)) as i64
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() % items.len() as u64) as usize]
    }
}

/// Strips llvm-objdump's `<symbol+off>` annotations and normalizes spaces.
pub fn normalize_objdump(text: &str) -> String {
    let mut s = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            _ => s.push(c),
        }
    }
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" ,", ",")
}

/// Compares Strata's and LLVM's text, allowing LLVM to name a CSR that
/// Strata does not implement (Strata prints it as a number), and LLVM's
/// `lpad imm` spelling of `auipc zero, imm` (the Zicfilp landing pad, which
/// llvm-objdump prints even when Zicfilp is not enabled; on Strata it is an
/// ordinary `auipc` to `x0`).
pub fn objdump_text_matches(ours: &str, theirs: &str, insn: &strata_isa::Insn) -> bool {
    use strata_isa::{csr_exists, Op};
    if ours == theirs {
        return true;
    }
    if insn.op == Op::Auipc
        && insn.rd == 0
        && theirs.strip_prefix("lpad ") == ours.strip_prefix("auipc zero, ")
    {
        return true;
    }
    // Likewise `ori zero, rs, imm` with imm[4:0] = 0, 1 or 3 is printed as the
    // Zicbop `prefetch.i/r/w (imm & ~31)(rs)`.
    if insn.op == Op::Ori && insn.rd == 0 {
        let kind = match insn.imm & 31 {
            0 => "i",
            1 => "r",
            3 => "w",
            _ => return false,
        };
        let off = insn.imm & !31;
        let off = if off < 0 {
            format!("-0x{:x}", -off)
        } else {
            format!("0x{off:x}")
        };
        return theirs == format!("prefetch.{kind} {off}({})", strata_isa::reg_name(insn.rs1));
    }
    if !insn.op.is_csr() || csr_exists(insn.csr) {
        return false;
    }
    let number = format!("0x{:x}", insn.csr);
    let split = |s: &str| {
        s.split([' ', ','])
            .filter(|t| !t.is_empty())
            .map(String::from)
            .collect::<Vec<_>>()
    };
    let (a, b) = (split(ours), split(theirs));
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x == y || *x == number)
}
