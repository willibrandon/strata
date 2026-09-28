//! General-purpose register names.

/// ABI names of `x0`..`x31`, indexed by register number.
const ABI_NAMES: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
    "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
    "t5", "t6",
];

/// Returns the ABI name of register `r` (`zero`, `ra`, `sp`, ..., `t6`).
///
/// Only the low five bits of `r` are used, so every input maps to a register.
pub fn reg_name(r: u8) -> &'static str {
    ABI_NAMES[usize::from(r & 31)]
}

/// Looks up a register by name, case-insensitively.
///
/// Accepts `x0`..`x31`, the ABI names (`zero ra sp gp tp t0-t6 s0-s11 a0-a7`),
/// and `fp` (an alias of `s0`/`x8`).
pub fn reg_by_name(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    let name = lower.as_str();
    if let Some(num) = name.strip_prefix('x') {
        // Reject forms like "x01" or "x+1" that `parse` would otherwise accept.
        let canonical = !num.is_empty()
            && num.bytes().all(|b| b.is_ascii_digit())
            && (num == "0" || !num.starts_with('0'));
        if canonical {
            if let Ok(n) = num.parse::<u8>() {
                if n < 32 {
                    return Some(n);
                }
            }
        }
        return None;
    }
    if name == "fp" {
        return Some(8);
    }
    ABI_NAMES.iter().position(|&n| n == name).map(|i| i as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for r in 0..32u8 {
            assert_eq!(reg_by_name(reg_name(r)), Some(r));
            assert_eq!(reg_by_name(&format!("x{r}")), Some(r));
        }
        assert_eq!(reg_by_name("fp"), Some(8));
        assert_eq!(reg_by_name("FP"), Some(8));
        assert_eq!(reg_by_name("A0"), Some(10));
        assert_eq!(reg_by_name("x32"), None);
        assert_eq!(reg_by_name("x01"), None);
        assert_eq!(reg_by_name("x"), None);
        assert_eq!(reg_by_name("s12"), None);
        assert_eq!(reg_name(10), "a0");
        assert_eq!(reg_name(42), "a0");
    }
}
