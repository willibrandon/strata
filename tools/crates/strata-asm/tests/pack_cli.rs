//! Test (d), strata-pack, plus end-to-end tests of both command-line tools.

mod common;

use common::{run_status, scratch, Rng};
use std::process::Command;
use strata_asm::pack::{build_image, crc32, parse_image, trim_segment, Segment, MAGIC};

const STRATA_AS: &str = env!("CARGO_BIN_EXE_strata-as");
const STRATA_PACK: &str = env!("CARGO_BIN_EXE_strata-pack");

#[test]
fn crc32_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    // Incremental property: CRC of the image header with hcrc zeroed matches.
    let img = build_image(
        0x1000_0000,
        &[Segment {
            addr: 0x1000_0000,
            data: vec![0x13, 0, 0, 0],
        }],
    )
    .unwrap();
    let mut hdr = img[..0x20].to_vec();
    let stored = u32::from_le_bytes([hdr[0x10], hdr[0x11], hdr[0x12], hdr[0x13]]);
    hdr[0x10..0x14].fill(0);
    assert_eq!(crc32(&hdr), stored);
    assert_eq!(
        u32::from_le_bytes([img[0x1C], img[0x1D], img[0x1E], img[0x1F]]),
        crc32(&[0x13, 0, 0, 0])
    );
}

#[test]
fn random_images_round_trip() {
    let mut rng = Rng::new(0xB007);
    for _ in 0..200 {
        let n = rng.range(1, 15) as usize;
        let mut addr = (rng.range(0, 0x1000) as u32) * 4;
        let mut segs = Vec::new();
        for _ in 0..n {
            let len = rng.range(0, 64) as usize * 4;
            let data: Vec<u8> = (0..len).map(|_| rng.next_u32() as u8).collect();
            segs.push(Segment { addr, data });
            addr += len as u32 + rng.range(0, 16) as u32 * 4;
        }
        let entry = rng.next_u32();
        let img = build_image(entry, &segs).unwrap();
        assert_eq!(u32::from_le_bytes([img[0], img[1], img[2], img[3]]), MAGIC);
        let parsed = parse_image(&img).unwrap();
        assert_eq!(parsed.entry, entry);
        assert_eq!(parsed.segments, segs);
    }
}

#[test]
fn trailing_zero_words_are_trimmed() {
    assert_eq!(
        trim_segment(&[1, 2, 3, 4, 0, 0, 0, 0, 0, 0]),
        vec![1, 2, 3, 4]
    );
    assert_eq!(trim_segment(&[0, 0, 0, 1, 0, 0, 0, 0]), vec![0, 0, 0, 1]);
}

fn write(path: &std::path::Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

#[test]
fn assemble_then_pack_end_to_end() {
    let dir = scratch("pack_cli_e2e");
    let src = dir.join("boot.s");
    write(
        &src,
        ".globl _start\n_start:\n  li a0, 1\n  j _start\n.data\n.word 0x11223344\n.zero 12\n.bss\n.zero 64\n",
    );
    let (bin, sdb, hex) = (
        dir.join("boot.bin"),
        dir.join("boot.sdb"),
        dir.join("boot.hex"),
    );
    let (ok, _, err) = run_status(
        Command::new(STRATA_AS)
            .args(["--org", "0x10000000", "--bin"])
            .arg(&bin)
            .arg("--debug")
            .arg(&sdb)
            .arg("--hex")
            .arg(&hex)
            .arg(&src),
    );
    assert!(ok, "{err}");
    let raw = std::fs::read(&bin).unwrap();
    assert_eq!(raw.len(), 8 + 16, "text + data, .bss excluded");

    let font = dir.join("font.bin");
    std::fs::write(&font, [7u8, 7, 7]).unwrap();
    let img = dir.join("system.img");
    let (ok, _, err) = run_status(
        Command::new(STRATA_PACK)
            .arg("-o")
            .arg(&img)
            .arg("--entry")
            .arg(format!("_start@{}", sdb.display()))
            .arg(format!("{}@0x10000000", bin.display()))
            .arg(format!("{}@0x20010000", font.display())),
    );
    assert!(ok, "{err}");
    let image = parse_image(&std::fs::read(&img).unwrap()).unwrap();
    assert_eq!(image.entry, 0x1000_0000);
    assert_eq!(image.segments.len(), 2);
    assert_eq!(image.segments[0].addr, 0x1000_0000);
    assert_eq!(
        image.segments[0].data,
        raw[..12].to_vec(),
        "trailing zero words trimmed"
    );
    assert_eq!(image.segments[1].data, vec![7, 7, 7, 0], "padded to a word");

    // Numeric entry, and errors.
    let (ok, _, _) = run_status(
        Command::new(STRATA_PACK)
            .arg("-o")
            .arg(&img)
            .args(["--entry", "0x10000004"])
            .arg(format!("{}@0x10000000", bin.display())),
    );
    assert!(ok);
    assert_eq!(
        parse_image(&std::fs::read(&img).unwrap()).unwrap().entry,
        0x1000_0004
    );
    let fail = |args: &[String], needle: &str| {
        let (ok, _, err) = run_status(Command::new(STRATA_PACK).args(args));
        assert!(!ok, "{args:?} should fail");
        assert!(
            err.contains(needle),
            "{args:?}: expected {needle:?} in {err}"
        );
    };
    let s = |v: &str| v.to_string();
    let seg = format!("{}@0x10000000", bin.display());
    fail(
        &[
            s("-o"),
            s("x.img"),
            s("--entry"),
            s("0"),
            format!("{}@0x10000002", bin.display()),
        ],
        "not 4-byte aligned",
    );
    fail(
        &[
            s("-o"),
            s("x.img"),
            s("--entry"),
            format!("nosuch@{}", sdb.display()),
            seg.clone(),
        ],
        "symbol 'nosuch' not found",
    );
    fail(
        &[
            s("-o"),
            s("x.img"),
            s("--entry"),
            s("0"),
            s("missing.bin@0"),
        ],
        "cannot read 'missing.bin'",
    );
    fail(
        &[
            s("-o"),
            s("x.img"),
            s("--entry"),
            s("0"),
            seg.clone(),
            seg.clone(),
        ],
        "overlap",
    );
    fail(&[s("--entry"), s("0"), seg.clone()], "no output file");
    let many: Vec<String> = (0..16)
        .map(|i| format!("{}@{:#x}", font.display(), 0x1000 * i))
        .collect();
    fail(
        &[vec![s("-o"), s("x.img"), s("--entry"), s("0")], many].concat(),
        "1 to 15 segments",
    );
}

#[test]
fn strata_as_command_line() {
    let dir = scratch("strata_as_cli");
    let (ok, _, err) = run_status(&mut Command::new(STRATA_AS));
    assert!(!ok && err.contains("usage: strata-as"), "{err}");
    let (ok, out, _) = run_status(Command::new(STRATA_AS).arg("--help"));
    assert!(ok && out.contains("--layout"));

    let bad = dir.join("bad.s");
    write(&bad, "nop\n  addi a0, a0, 99999\n  frob a1\n");
    let (ok, _, err) = run_status(Command::new(STRATA_AS).args(["--org", "0"]).arg(&bad));
    assert!(!ok);
    let path = bad.display().to_string();
    assert!(err.contains(&format!("{path}:2:16: error:")), "{err}");
    assert!(
        err.contains(&format!("{path}:3:3: error: unknown instruction 'frob'")),
        "{err}"
    );

    let good = dir.join("good.s");
    let layout = dir.join("l.ld");
    write(&good, ".globl _start\n_start: nop\n.data\n.word VALUE\n");
    write(&layout, "memory M origin=0x100 length=1K\noutput .text in M { .text }\noutput .data in M { .data }\nentry _start\n");
    let outs = ["o.elf", "o.bin", "o.hex", "o.sdb", "o.lst", "o.map"].map(|n| dir.join(n));
    let (ok, _, err) = run_status(
        Command::new(STRATA_AS)
            .arg("-T")
            .arg(&layout)
            .arg("-DVALUE=0x55")
            .arg("-o")
            .arg(&outs[0])
            .arg("--bin")
            .arg(&outs[1])
            .arg("--hex")
            .arg(&outs[2])
            .arg("--debug")
            .arg(&outs[3])
            .arg("--listing")
            .arg(&outs[4])
            .arg("--map")
            .arg(&outs[5])
            .arg(&good),
    );
    assert!(ok, "{err}");
    assert_eq!(
        std::fs::read(&outs[1]).unwrap(),
        vec![0x13, 0, 0, 0, 0x55, 0, 0, 0]
    );
    assert_eq!(
        std::fs::read_to_string(&outs[2]).unwrap(),
        "00000013\n00000055\n"
    );
    for o in &outs {
        assert!(
            std::fs::metadata(o).unwrap().len() > 0,
            "{} is empty",
            o.display()
        );
    }
    // Layout errors are reported with their location.
    write(&layout, "memory M origin=0x100\n");
    let (ok, _, err) = run_status(Command::new(STRATA_AS).arg("-T").arg(&layout).arg(&good));
    assert!(
        !ok && err.contains(&format!(
            "{}:1:1: error: memory requires origin= and length=",
            layout.display()
        )),
        "{err}"
    );
    let (ok, _, err) = run_status(Command::new(STRATA_AS).args(["--org", "0", "/nonexistent/x.s"]));
    assert!(!ok && err.contains("cannot read input file"), "{err}");
}
