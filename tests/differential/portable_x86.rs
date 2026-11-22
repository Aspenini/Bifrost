//! ISA-independent expectations for hosts without native x86-64 instructions.

use super::{Arithmetic, Branch};

pub(super) fn arithmetic(op: Arithmetic, lhs: u64, rhs: u64) -> (u64, u64) {
    let (result, carry, nibble_carry, overflow) = match op {
        Arithmetic::Add => {
            let (result, carry) = lhs.overflowing_add(rhs);
            let overflow = (lhs as i64).overflowing_add(rhs as i64).1;
            (result, carry, (lhs & 15) + (rhs & 15) > 15, overflow)
        }
        Arithmetic::Sub | Arithmetic::Cmp => {
            let (result, carry) = lhs.overflowing_sub(rhs);
            let overflow = (lhs as i64).overflowing_sub(rhs as i64).1;
            (result, carry, (lhs & 15) < (rhs & 15), overflow)
        }
    };
    let mut flags = 0;
    for (bit, set) in [
        (0, carry),
        (2, (result as u8).count_ones().is_multiple_of(2)),
        (4, nibble_carry),
        (6, result == 0),
        (7, (result as i64).is_negative()),
        (11, overflow),
    ] {
        flags |= u64::from(set) << bit;
    }
    let rax = if matches!(op, Arithmetic::Cmp) {
        lhs
    } else {
        result
    };
    (rax, flags)
}

pub(super) fn branch(condition: Branch, lhs: u64, rhs: u64) -> bool {
    match condition {
        Branch::Equal => lhs == rhs,
        Branch::NotEqual => lhs != rhs,
        Branch::Below => lhs < rhs,
        Branch::AboveOrEqual => lhs >= rhs,
        Branch::Less => (lhs as i64) < (rhs as i64),
        Branch::GreaterOrEqual => (lhs as i64) >= (rhs as i64),
    }
}

pub(super) fn move_and_add() -> (u64, [u8; 8], u64) {
    let (rax, flags) = arithmetic(Arithmetic::Add, 41, 1);
    (rax, 41_u64.to_le_bytes(), flags)
}

pub(super) fn indexed_immediate_store() -> (u64, [u8; 8]) {
    (u64::MAX - 1, (-2_i64).to_le_bytes())
}

pub(super) fn move_and_jump() -> u64 {
    0x0123_4567_89ab_cdef - 2
}
