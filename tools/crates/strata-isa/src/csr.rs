//! Control and status registers implemented by the Strata S1 (see `docs/isa.md`).

/// Single (non-range) CSRs: address and canonical name.
const SINGLE: [(u16, &str); 29] = [
    (0x300, "mstatus"),
    (0x301, "misa"),
    (0x304, "mie"),
    (0x305, "mtvec"),
    (0x306, "mcounteren"),
    (0x30A, "menvcfg"),
    (0x310, "mstatush"),
    (0x31A, "menvcfgh"),
    (0x320, "mcountinhibit"),
    (0x340, "mscratch"),
    (0x341, "mepc"),
    (0x342, "mcause"),
    (0x343, "mtval"),
    (0x344, "mip"),
    (0xB00, "mcycle"),
    (0xB02, "minstret"),
    (0xB80, "mcycleh"),
    (0xB82, "minstreth"),
    (0xC00, "cycle"),
    (0xC01, "time"),
    (0xC02, "instret"),
    (0xC80, "cycleh"),
    (0xC81, "timeh"),
    (0xC82, "instreth"),
    (0xF11, "mvendorid"),
    (0xF12, "marchid"),
    (0xF13, "mimpid"),
    (0xF14, "mhartid"),
    (0xF15, "mconfigptr"),
];

const MHPMEVENT: [&str; 29] = [
    "mhpmevent3",
    "mhpmevent4",
    "mhpmevent5",
    "mhpmevent6",
    "mhpmevent7",
    "mhpmevent8",
    "mhpmevent9",
    "mhpmevent10",
    "mhpmevent11",
    "mhpmevent12",
    "mhpmevent13",
    "mhpmevent14",
    "mhpmevent15",
    "mhpmevent16",
    "mhpmevent17",
    "mhpmevent18",
    "mhpmevent19",
    "mhpmevent20",
    "mhpmevent21",
    "mhpmevent22",
    "mhpmevent23",
    "mhpmevent24",
    "mhpmevent25",
    "mhpmevent26",
    "mhpmevent27",
    "mhpmevent28",
    "mhpmevent29",
    "mhpmevent30",
    "mhpmevent31",
];
const PMPCFG: [&str; 4] = ["pmpcfg0", "pmpcfg1", "pmpcfg2", "pmpcfg3"];
const PMPADDR: [&str; 16] = [
    "pmpaddr0",
    "pmpaddr1",
    "pmpaddr2",
    "pmpaddr3",
    "pmpaddr4",
    "pmpaddr5",
    "pmpaddr6",
    "pmpaddr7",
    "pmpaddr8",
    "pmpaddr9",
    "pmpaddr10",
    "pmpaddr11",
    "pmpaddr12",
    "pmpaddr13",
    "pmpaddr14",
    "pmpaddr15",
];
const MHPMCOUNTER: [&str; 29] = [
    "mhpmcounter3",
    "mhpmcounter4",
    "mhpmcounter5",
    "mhpmcounter6",
    "mhpmcounter7",
    "mhpmcounter8",
    "mhpmcounter9",
    "mhpmcounter10",
    "mhpmcounter11",
    "mhpmcounter12",
    "mhpmcounter13",
    "mhpmcounter14",
    "mhpmcounter15",
    "mhpmcounter16",
    "mhpmcounter17",
    "mhpmcounter18",
    "mhpmcounter19",
    "mhpmcounter20",
    "mhpmcounter21",
    "mhpmcounter22",
    "mhpmcounter23",
    "mhpmcounter24",
    "mhpmcounter25",
    "mhpmcounter26",
    "mhpmcounter27",
    "mhpmcounter28",
    "mhpmcounter29",
    "mhpmcounter30",
    "mhpmcounter31",
];
const MHPMCOUNTERH: [&str; 29] = [
    "mhpmcounter3h",
    "mhpmcounter4h",
    "mhpmcounter5h",
    "mhpmcounter6h",
    "mhpmcounter7h",
    "mhpmcounter8h",
    "mhpmcounter9h",
    "mhpmcounter10h",
    "mhpmcounter11h",
    "mhpmcounter12h",
    "mhpmcounter13h",
    "mhpmcounter14h",
    "mhpmcounter15h",
    "mhpmcounter16h",
    "mhpmcounter17h",
    "mhpmcounter18h",
    "mhpmcounter19h",
    "mhpmcounter20h",
    "mhpmcounter21h",
    "mhpmcounter22h",
    "mhpmcounter23h",
    "mhpmcounter24h",
    "mhpmcounter25h",
    "mhpmcounter26h",
    "mhpmcounter27h",
    "mhpmcounter28h",
    "mhpmcounter29h",
    "mhpmcounter30h",
    "mhpmcounter31h",
];
const HPMCOUNTER: [&str; 29] = [
    "hpmcounter3",
    "hpmcounter4",
    "hpmcounter5",
    "hpmcounter6",
    "hpmcounter7",
    "hpmcounter8",
    "hpmcounter9",
    "hpmcounter10",
    "hpmcounter11",
    "hpmcounter12",
    "hpmcounter13",
    "hpmcounter14",
    "hpmcounter15",
    "hpmcounter16",
    "hpmcounter17",
    "hpmcounter18",
    "hpmcounter19",
    "hpmcounter20",
    "hpmcounter21",
    "hpmcounter22",
    "hpmcounter23",
    "hpmcounter24",
    "hpmcounter25",
    "hpmcounter26",
    "hpmcounter27",
    "hpmcounter28",
    "hpmcounter29",
    "hpmcounter30",
    "hpmcounter31",
];
const HPMCOUNTERH: [&str; 29] = [
    "hpmcounter3h",
    "hpmcounter4h",
    "hpmcounter5h",
    "hpmcounter6h",
    "hpmcounter7h",
    "hpmcounter8h",
    "hpmcounter9h",
    "hpmcounter10h",
    "hpmcounter11h",
    "hpmcounter12h",
    "hpmcounter13h",
    "hpmcounter14h",
    "hpmcounter15h",
    "hpmcounter16h",
    "hpmcounter17h",
    "hpmcounter18h",
    "hpmcounter19h",
    "hpmcounter20h",
    "hpmcounter21h",
    "hpmcounter22h",
    "hpmcounter23h",
    "hpmcounter24h",
    "hpmcounter25h",
    "hpmcounter26h",
    "hpmcounter27h",
    "hpmcounter28h",
    "hpmcounter29h",
    "hpmcounter30h",
    "hpmcounter31h",
];

/// Contiguous CSR ranges: first address and the names of the whole range.
const RANGES: [(u16, &[&str]); 7] = [
    (0x323, &MHPMEVENT),
    (0x3A0, &PMPCFG),
    (0x3B0, &PMPADDR),
    (0xB03, &MHPMCOUNTER),
    (0xB83, &MHPMCOUNTERH),
    (0xC03, &HPMCOUNTER),
    (0xC83, &HPMCOUNTERH),
];

/// Returns the canonical name of the CSR at `addr`, if Strata implements it.
pub fn csr_name(addr: u16) -> Option<&'static str> {
    if let Some(&(_, name)) = SINGLE.iter().find(|&&(a, _)| a == addr) {
        return Some(name);
    }
    RANGES.iter().find_map(|&(base, names)| {
        let idx = usize::from(addr.checked_sub(base)?);
        names.get(idx).copied()
    })
}

/// Returns true if `addr` names a CSR that exists on Strata.
///
/// Existence is independent of privilege: whether an access is legal also
/// depends on the current mode, `mcounteren`, and whether it writes (see
/// [`csr_privilege`] and [`csr_is_read_only`]).
pub fn csr_exists(addr: u16) -> bool {
    csr_name(addr).is_some()
}

/// Looks up a CSR by name (case-insensitive) among the CSRs Strata implements.
pub fn csr_by_name(name: &str) -> Option<u16> {
    let lower = name.to_ascii_lowercase();
    if let Some(&(addr, _)) = SINGLE.iter().find(|&&(_, n)| n == lower) {
        return Some(addr);
    }
    RANGES.iter().find_map(|&(base, names)| {
        names
            .iter()
            .position(|&n| n == lower)
            .map(|i| base + i as u16)
    })
}

/// Lowest privilege level that may access the CSR (`addr[9:8]`: 0 = U, 3 = M).
pub fn csr_privilege(addr: u16) -> u8 {
    ((addr >> 8) & 3) as u8
}

/// True if the CSR address is in the read-only space (`addr[11:10] == 0b11`).
pub fn csr_is_read_only(addr: u16) -> bool {
    (addr >> 10) & 3 == 3
}

/// All implemented CSR addresses in ascending order.
pub fn all_csrs() -> Vec<u16> {
    (0u16..4096).filter(|&a| csr_exists(a)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_isa_md() {
        let expect_single = [
            0x300, 0x301, 0x304, 0x305, 0x306, 0x30A, 0x310, 0x31A, 0x320, 0x340, 0x341, 0x342,
            0x343, 0x344, 0xB00, 0xB02, 0xB80, 0xB82, 0xC00, 0xC01, 0xC02, 0xC80, 0xC81, 0xC82,
            0xF11, 0xF12, 0xF13, 0xF14, 0xF15,
        ];
        let mut expect: Vec<u16> = expect_single.to_vec();
        expect.extend(0x323..=0x33F);
        expect.extend(0x3A0..=0x3A3);
        expect.extend(0x3B0..=0x3BF);
        expect.extend(0xB03..=0xB1F);
        expect.extend(0xB83..=0xB9F);
        expect.extend(0xC03..=0xC1F);
        expect.extend(0xC83..=0xC9F);
        expect.sort_unstable();
        assert_eq!(all_csrs(), expect);
    }

    #[test]
    fn names_round_trip() {
        for addr in all_csrs() {
            let name = csr_name(addr).unwrap();
            assert_eq!(csr_by_name(name), Some(addr), "{name}");
        }
        assert_eq!(csr_by_name("MSTATUS"), Some(0x300));
        assert_eq!(csr_by_name("mhpmcounter31h"), Some(0xB9F));
        assert_eq!(csr_by_name("hpmcounter3"), Some(0xC03));
        assert_eq!(csr_by_name("pmpaddr15"), Some(0x3BF));
        assert_eq!(csr_by_name("pmpcfg3"), Some(0x3A3));
        assert_eq!(csr_by_name("mhpmevent31"), Some(0x33F));
        assert_eq!(csr_by_name("sstatus"), None);
        assert_eq!(csr_by_name(""), None);
        assert_eq!(csr_name(0x100), None);
        assert_eq!(csr_name(0x3C0), None);
        assert_eq!(csr_name(0xFFF), None);
    }

    #[test]
    fn privilege_bits() {
        assert_eq!(csr_privilege(0x300), 3);
        assert_eq!(csr_privilege(0xC00), 0);
        assert!(csr_is_read_only(0xC00));
        assert!(csr_is_read_only(0xF14));
        assert!(!csr_is_read_only(0xB00));
    }
}
