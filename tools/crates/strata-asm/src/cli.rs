//! Command-line front ends of `strata-as` and `strata-pack`.

use crate::output::{flat_binary, hex_words, listing, map, write_elf};
use crate::pack::{build_image, trim_segment, Segment};
use crate::sdb::DebugInfo;
use crate::{assemble_and_link, LayoutSpec, Options};
use std::path::PathBuf;

/// Parses an integer: decimal, `0x` hex, `0b` binary or `0o` octal, with an
/// optional sign and `_` separators.
pub fn parse_int(text: &str) -> Option<i64> {
    let t: String = text.trim().chars().filter(|&c| c != '_').collect();
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r.to_string()),
        None => (false, t.strip_prefix('+').unwrap_or(&t).to_string()),
    };
    let lower = t.to_ascii_lowercase();
    let v = if let Some(h) = lower.strip_prefix("0x") {
        u64::from_str_radix(h, 16).ok()?
    } else if let Some(b) = lower.strip_prefix("0b") {
        u64::from_str_radix(b, 2).ok()?
    } else if let Some(o) = lower.strip_prefix("0o") {
        u64::from_str_radix(o, 8).ok()?
    } else {
        lower.parse::<u64>().ok()?
    };
    let v = v as i64;
    Some(if neg { v.wrapping_neg() } else { v })
}

/// Parses a 32-bit address.
pub fn parse_addr(text: &str) -> Option<u32> {
    parse_int(text)
        .filter(|v| (0..1i64 << 32).contains(v))
        .map(|v| v as u32)
}

/// Parsed `strata-as` command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AsArgs {
    pub inputs: Vec<String>,
    pub layout: Option<String>,
    pub org: Option<u32>,
    pub elf: Option<String>,
    pub bin: Option<String>,
    pub hex: Option<String>,
    pub debug: Option<String>,
    pub listing: Option<String>,
    pub map: Option<String>,
    pub include_dirs: Vec<String>,
    pub defines: Vec<(String, i64)>,
    pub gc_sections: bool,
    pub keep_symbols: Vec<String>,
    pub help: bool,
    pub version: bool,
}

pub const AS_USAGE: &str = "\
usage: strata-as [options] <input.s>...
  -T, --layout <file>     layout file (required unless --org is given)
      --org <addr>        simple layout: everything contiguous from <addr>
  -o, --elf <file>        ELF32 executable
      --bin <file>        flat binary of all loaded bytes
      --hex <file>        like --bin, one 32-bit word per line ($readmemh)
      --debug <file>      debug information (.sdb JSON)
      --listing <file>    listing: address, encoding, source text
      --map <file>        placement of every input section and symbol
      --gc-sections       remove input sections not reachable from the entry
                          symbol, KEEP() layout patterns or --keep-symbol
      --keep-symbol <sym> with --gc-sections, keep the section defining <sym>
                          (repeatable)
  -I <dir>                search directory for .include / .incbin
  -D <sym>=<value>        define an absolute symbol before assembly
  -h, --help              show this help
  -V, --version           show the version
";

/// Parses `strata-as` arguments (without the program name).
pub fn parse_as_args(args: &[String]) -> Result<AsArgs, String> {
    let mut a = AsArgs::default();
    let mut i = 0;
    let mut only_inputs = false;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if only_inputs || !arg.starts_with('-') || arg == "-" {
            a.inputs.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_inputs = true;
            continue;
        }
        // Split "--opt=value" and "-Xvalue".
        let (name, inline): (&str, Option<String>) = if let Some(rest) = arg.strip_prefix("--") {
            match rest.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (rest, None),
            }
        } else if arg.len() > 2 {
            (&arg[1..2], Some(arg[2..].to_string()))
        } else {
            (&arg[1..], None)
        };
        let mut value = |what: &str| -> Result<String, String> {
            if let Some(v) = inline.clone() {
                return Ok(v);
            }
            let v = args
                .get(i)
                .cloned()
                .ok_or(format!("option '{arg}' requires {what}"))?;
            i += 1;
            Ok(v)
        };
        match name {
            "h" | "help" => a.help = true,
            "V" | "version" => a.version = true,
            "gc-sections" if inline.is_none() => a.gc_sections = true,
            "keep-symbol" => a.keep_symbols.push(value("a symbol name")?),
            "T" | "layout" => a.layout = Some(value("a file name")?),
            "org" => {
                let v = value("an address")?;
                a.org = Some(parse_addr(&v).ok_or(format!("invalid address '{v}'"))?);
            }
            "o" | "elf" => a.elf = Some(value("a file name")?),
            "bin" => a.bin = Some(value("a file name")?),
            "hex" => a.hex = Some(value("a file name")?),
            "debug" => a.debug = Some(value("a file name")?),
            "listing" => a.listing = Some(value("a file name")?),
            "map" => a.map = Some(value("a file name")?),
            "I" => a.include_dirs.push(value("a directory")?),
            "D" => {
                let v = value("sym=value")?;
                let (sym, val) = match v.split_once('=') {
                    Some((s, n)) => (
                        s.to_string(),
                        parse_int(n).ok_or(format!("invalid value in '-D {v}'"))?,
                    ),
                    None => (v.clone(), 1),
                };
                let valid = sym.bytes().next().is_some_and(crate::lexer::is_ident_start)
                    && sym.bytes().all(crate::lexer::is_ident_char);
                if !valid {
                    return Err(format!("invalid symbol name in '-D {v}'"));
                }
                a.defines.push((sym, val));
            }
            _ => return Err(format!("unknown option '{arg}'")),
        }
    }
    if a.help || a.version {
        return Ok(a);
    }
    if a.inputs.is_empty() {
        return Err("no input files".to_string());
    }
    match (&a.layout, a.org) {
        (None, None) => return Err("a layout file (-T) or --org is required".to_string()),
        (Some(_), Some(_)) => return Err("-T and --org cannot be used together".to_string()),
        _ => {}
    }
    Ok(a)
}

fn write_file(path: &str, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("cannot write '{path}': {e}"))
}

/// Runs `strata-as` with `args` (without the program name). Returns the exit
/// status; diagnostics go to standard error.
pub fn run_as(args: &[String]) -> i32 {
    let a = match parse_as_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("strata-as: error: {e}\n{AS_USAGE}");
            return 2;
        }
    };
    if a.help {
        print!("{AS_USAGE}");
        return 0;
    }
    if a.version {
        println!("strata-as {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let opts = Options {
        inputs: a.inputs.clone(),
        layout: match (&a.layout, a.org) {
            (Some(f), _) => LayoutSpec::File(f.clone()),
            (None, Some(o)) => LayoutSpec::Org(o),
            (None, None) => LayoutSpec::Org(0),
        },
        include_dirs: a.include_dirs.iter().map(PathBuf::from).collect(),
        defines: a.defines.clone(),
        gc_sections: a.gc_sections,
        keep_symbols: a.keep_symbols.clone(),
    };
    let out = assemble_and_link(&opts);
    eprint!("{}", out.diags.render(&out.files, "strata-as"));
    let Some(p) = out.program else {
        return 1;
    };
    let result = (|| -> Result<(), String> {
        if let Some(path) = &a.elf {
            write_file(path, &write_elf(&p))?;
        }
        if a.bin.is_some() || a.hex.is_some() {
            let (_, bin) = flat_binary(&p)?;
            if let Some(path) = &a.bin {
                write_file(path, &bin)?;
            }
            if let Some(path) = &a.hex {
                write_file(path, hex_words(&bin).as_bytes())?;
            }
        }
        if let Some(path) = &a.debug {
            write_file(
                path,
                DebugInfo::from_program(&p, &out.files).to_json().as_bytes(),
            )?;
        }
        if let Some(path) = &a.listing {
            write_file(path, listing(&p, &out.files).as_bytes())?;
        }
        if let Some(path) = &a.map {
            write_file(path, map(&p).as_bytes())?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("strata-as: error: {e}");
            1
        }
    }
}

pub const PACK_USAGE: &str = "\
usage: strata-pack -o <image> --entry <addr|symbol@file.sdb> <file.bin>@<addr>...
  -o, --output <file>     boot image to write
  -e, --entry <entry>     entry address, or symbol@file.sdb
  -h, --help              show this help
";

/// Parsed `strata-pack` command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackArgs {
    pub output: String,
    pub entry: String,
    /// `(file, load address)`.
    pub segments: Vec<(String, u32)>,
    pub help: bool,
}

/// Parses `strata-pack` arguments (without the program name).
pub fn parse_pack_args(args: &[String]) -> Result<PackArgs, String> {
    let mut a = PackArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].clone();
        i += 1;
        let value = |i: &mut usize| -> Result<String, String> {
            if let Some((_, v)) = arg.split_once('=').filter(|_| arg.starts_with("--")) {
                return Ok(v.to_string());
            }
            let v = args
                .get(*i)
                .cloned()
                .ok_or(format!("option '{arg}' requires a value"))?;
            *i += 1;
            Ok(v)
        };
        let name = arg.split('=').next().unwrap_or("");
        match name {
            "-h" | "--help" => a.help = true,
            "-o" | "--output" => a.output = value(&mut i)?,
            "-e" | "--entry" => a.entry = value(&mut i)?,
            _ if arg.starts_with('-') => return Err(format!("unknown option '{arg}'")),
            _ => {
                let (file, addr) = arg
                    .rsplit_once('@')
                    .ok_or(format!("segment '{arg}' must be <file>@<addr>"))?;
                let addr = parse_addr(addr).ok_or(format!("invalid address in '{arg}'"))?;
                a.segments.push((file.to_string(), addr));
            }
        }
    }
    if a.help {
        return Ok(a);
    }
    if a.output.is_empty() {
        return Err("no output file (-o)".to_string());
    }
    if a.entry.is_empty() {
        return Err("no entry point (--entry)".to_string());
    }
    if a.segments.is_empty() {
        return Err("no segments".to_string());
    }
    Ok(a)
}

/// Resolves `--entry`: a number, or `symbol@file.sdb`.
pub fn resolve_entry(entry: &str) -> Result<u32, String> {
    if let Some(v) = parse_addr(entry) {
        return Ok(v);
    }
    let (sym, file) = entry.rsplit_once('@').ok_or(format!(
        "invalid entry '{entry}' (expected an address or symbol@file.sdb)"
    ))?;
    let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read '{file}': {e}"))?;
    let info = DebugInfo::from_json(&text).map_err(|e| format!("{file}: {e}"))?;
    info.symbol_addr(sym).map_err(|e| format!("{file}: {e}"))
}

/// Runs `strata-pack`; returns the exit status.
pub fn run_pack(args: &[String]) -> i32 {
    let a = match parse_pack_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("strata-pack: error: {e}\n{PACK_USAGE}");
            return 2;
        }
    };
    if a.help {
        print!("{PACK_USAGE}");
        return 0;
    }
    let result = (|| -> Result<(), String> {
        let entry = resolve_entry(&a.entry)?;
        let mut segments = Vec::new();
        for (file, addr) in &a.segments {
            let data = std::fs::read(file).map_err(|e| format!("cannot read '{file}': {e}"))?;
            segments.push(Segment {
                addr: *addr,
                data: trim_segment(&data),
            });
        }
        let image = build_image(entry, &segments)?;
        write_file(&a.output, &image)
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("strata-pack: error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn integers() {
        assert_eq!(parse_int("0x1000_0000"), Some(0x1000_0000));
        assert_eq!(parse_int("-5"), Some(-5));
        assert_eq!(parse_int("0b101"), Some(5));
        assert_eq!(parse_int("12"), Some(12));
        assert_eq!(parse_int("x"), None);
        assert_eq!(parse_addr("0x100000000"), None);
        assert_eq!(parse_addr("0xFFFFFFFF"), Some(u32::MAX));
    }

    #[test]
    fn as_arguments() {
        let a = parse_as_args(&args(
            "-T l.ld -o a.elf --bin=a.bin -Iinc -I inc2 -DX=0x10 -D Y a.s b.s",
        ))
        .unwrap();
        assert_eq!(a.layout.as_deref(), Some("l.ld"));
        assert_eq!(a.elf.as_deref(), Some("a.elf"));
        assert_eq!(a.bin.as_deref(), Some("a.bin"));
        assert_eq!(a.include_dirs, vec!["inc", "inc2"]);
        assert_eq!(a.defines, vec![("X".to_string(), 16), ("Y".to_string(), 1)]);
        assert_eq!(a.inputs, vec!["a.s", "b.s"]);
        let o = parse_as_args(&args("--org 0x10000000 x.s")).unwrap();
        assert_eq!(o.org, Some(0x1000_0000));
        assert!(parse_as_args(&args("x.s")).is_err());
        assert!(parse_as_args(&args("--org 0 -T l x.s")).is_err());
        assert!(parse_as_args(&args("--org 0")).is_err());
        assert!(parse_as_args(&args("--org 0 --bogus x.s")).is_err());
        assert!(parse_as_args(&args("--org 0 -D 1x=2 x.s")).is_err());
        assert!(parse_as_args(&args("--org 0 x.s -o")).is_err());
        let g = parse_as_args(&args(
            "--org 0 --gc-sections --keep-symbol a --keep-symbol=b x.s",
        ))
        .unwrap();
        assert!(g.gc_sections);
        assert_eq!(g.keep_symbols, vec!["a", "b"]);
        assert!(parse_as_args(&args("--org 0 --gc-sections=yes x.s")).is_err());
        assert!(parse_as_args(&args("--org 0 x.s --keep-symbol")).is_err());
    }

    #[test]
    fn pack_arguments() {
        let a = parse_pack_args(&args(
            "-o sys.img --entry main@k.sdb k.bin@0x10000000 f.bin@0x20010000",
        ))
        .unwrap();
        assert_eq!(a.output, "sys.img");
        assert_eq!(a.entry, "main@k.sdb");
        assert_eq!(
            a.segments,
            vec![
                ("k.bin".to_string(), 0x1000_0000),
                ("f.bin".to_string(), 0x2001_0000)
            ]
        );
        assert!(parse_pack_args(&args("-o x --entry 0 k.bin")).is_err());
        assert!(parse_pack_args(&args("--entry 0 k.bin@0")).is_err());
        assert_eq!(resolve_entry("0x100").unwrap(), 0x100);
        assert!(resolve_entry("nosuch@/nonexistent.sdb").is_err());
    }
}
