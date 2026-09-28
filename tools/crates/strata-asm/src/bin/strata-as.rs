//! `strata-as`: Strata's assembler and linker. See `docs/toolchain.md`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(strata_asm::cli::run_as(&args));
}
