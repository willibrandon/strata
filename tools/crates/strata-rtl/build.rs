//! Builds the Verilator model of strata_soc from the RTL and links it. When
//! Verilator is not installed, a stub is linked and the engine reports itself
//! unavailable at run time.

use std::path::{Path, PathBuf};
use std::process::Command;

fn find_verilator() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VERILATOR") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let cad = std::env::var("OSS_CAD_SUITE").unwrap_or(format!("{home}/tools/oss-cad-suite"));
    let p = Path::new(&cad).join("bin/verilator");
    if p.exists() {
        return Some(p);
    }
    let out = Command::new("which").arg("verilator").output().ok()?;
    out.status.success().then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../..").canonicalize().unwrap();
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=cpp");
    println!("cargo:rerun-if-changed={}", root.join("rtl").display());
    println!("cargo:rerun-if-env-changed=VERILATOR");

    let files: Vec<PathBuf> = std::fs::read_to_string(root.join("rtl/files.f"))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| root.join(l.trim()))
        .collect();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
    }

    let Some(verilator) = find_verilator() else {
        println!("cargo:warning=verilator not found: RTL engine unavailable");
        cc::Build::new().file("cpp/stub.c").compile("strata_rtl");
        return;
    };

    let target = std::env::var("TARGET").unwrap();
    // Match the deployment target rustc links for, to avoid version warnings.
    let min_os = if target.contains("apple") { " -mmacosx-version-min=11.0" } else { "" };
    let cflags = format!("-O2 -fPIC{min_os}");
    let mdir = out.join("vobj");
    let status = Command::new(&verilator)
        .args(["--cc", "--build", "-O3", "--x-assign", "0", "--x-initial", "0", "-Wno-fatal", "-Wno-lint",
               "-Wno-style", "--top-module", "strata_soc", "--prefix", "Vstrata_soc", "-j", "0",
               "-CFLAGS", &cflags])
        .arg("-Mdir")
        .arg(&mdir)
        .args(&files)
        .status()
        .expect("running verilator");
    assert!(status.success(), "verilator failed");

    let vroot = Command::new(&verilator).arg("--getenv").arg("VERILATOR_ROOT").output().expect("verilator root");
    let vroot = PathBuf::from(String::from_utf8_lossy(&vroot.stdout).trim());
    let mut shim = cc::Build::new();
    if !min_os.is_empty() {
        shim.flag(min_os.trim());
    }
    shim.cpp(true)
        .warnings(false)
        .std("c++17")
        .flag("-O2")
        .include(&mdir)
        .include(vroot.join("include"))
        .include(vroot.join("include/vltstd"))
        .file("cpp/shim.cpp")
        .compile("strata_rtl");

    println!("cargo:rustc-link-search=native={}", mdir.display());
    println!("cargo:rustc-link-lib=static=Vstrata_soc");
    println!("cargo:rustc-link-lib=static=verilated");
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else {
        println!("cargo:rustc-link-lib=stdc++");
    }
}
