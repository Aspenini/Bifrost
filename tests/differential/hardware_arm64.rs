//! Fixed native ARM64 reference instructions. No guest buffer is executed by this oracle.

use core::arch::asm;

pub(super) fn arithmetic(lhs: u64, rhs: u64, sub: bool) -> (u64, u32) {
    macro_rules! check {
        ($instruction:literal) => {{
            let result: u64;
            let flags: u64;
            // SAFETY: All operands are integer registers. NZCV is a user-accessible register.
            unsafe {
                asm!(
                    concat!($instruction, " {result}, {lhs}, {rhs}"),
                    "mrs {flags}, nzcv",
                    lhs = in(reg) lhs, rhs = in(reg) rhs,
                    result = lateout(reg) result, flags = lateout(reg) flags,
                    options(nostack, nomem),
                );
            }
            (result, flags as u32)
        }};
    }
    if sub { check!("subs") } else { check!("adds") }
}

pub(super) fn branch(condition: usize, flags: u32) -> bool {
    macro_rules! check {
        ($suffix:literal) => {{
            let taken: u64;
            // SAFETY: Flags are restricted to NZCV and both branch targets stay inside this block.
            unsafe {
                asm!(
                    "msr nzcv, {flags}",
                    "mov {taken}, #0",
                    concat!("b.", $suffix, " 2f"),
                    "b 3f",
                    "2:",
                    "mov {taken}, #1",
                    "3:",
                    flags = in(reg) u64::from(flags),
                    taken = lateout(reg) taken,
                    options(nostack, nomem),
                );
            }
            taken == 1
        }};
    }
    match condition {
        0 => check!("eq"),
        1 => check!("ne"),
        2 => check!("cs"),
        3 => check!("cc"),
        4 => check!("mi"),
        5 => check!("pl"),
        6 => check!("vs"),
        7 => check!("vc"),
        8 => check!("hi"),
        9 => check!("ls"),
        10 => check!("ge"),
        11 => check!("lt"),
        12 => check!("gt"),
        13 => check!("le"),
        14 => check!("al"),
        15 => check!("nv"),
        _ => unreachable!("four-bit condition"),
    }
}
