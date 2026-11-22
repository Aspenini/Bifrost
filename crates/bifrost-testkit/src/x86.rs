//! Hand-authored x86-64 bytes and reference expectations.

/// Integer flags defined by ADD, SUB and CMP.
pub const ARITHMETIC_FLAGS: u64 = 0x8d5;

#[derive(Clone, Copy, Debug)]
pub struct ArithmeticCase {
    pub name: &'static str,
    pub bytes: &'static [u8],
    pub lhs: u64,
    pub rhs: u64,
    pub rax: u64,
    pub flags: u64,
}

/// Expected flags exclude reserved bit 1 and all unaffected flags.
pub const ARITHMETIC: &[ArithmeticCase] = &[
    ArithmeticCase {
        name: "add carry",
        bytes: &[0x48, 0x01, 0xd8],
        lhs: u64::MAX,
        rhs: 1,
        rax: 0,
        flags: 0x55,
    },
    ArithmeticCase {
        name: "add overflow",
        bytes: &[0x48, 0x03, 0xc3],
        lhs: i64::MAX as u64,
        rhs: 1,
        rax: 1 << 63,
        flags: 0x894,
    },
    ArithmeticCase {
        name: "sub borrow",
        bytes: &[0x48, 0x29, 0xd8],
        lhs: 0,
        rhs: 1,
        rax: u64::MAX,
        flags: 0x95,
    },
    ArithmeticCase {
        name: "sub overflow",
        bytes: &[0x48, 0x2b, 0xc3],
        lhs: 1 << 63,
        rhs: 1,
        rax: i64::MAX as u64,
        flags: 0x814,
    },
    ArithmeticCase {
        name: "cmp equal",
        bytes: &[0x48, 0x39, 0xd8],
        lhs: 42,
        rhs: 42,
        rax: 42,
        flags: 0x44,
    },
    ArithmeticCase {
        name: "cmp signed less",
        bytes: &[0x48, 0x3b, 0xc3],
        lhs: u64::MAX,
        rhs: 1,
        rax: u64::MAX,
        flags: 0x80,
    },
    ArithmeticCase {
        name: "add imm8 sign extension",
        bytes: &[0x48, 0x83, 0xc0, 0xff],
        lhs: 1,
        rhs: 0,
        rax: 0,
        flags: 0x55,
    },
    ArithmeticCase {
        name: "sub accumulator imm32 sign extension",
        bytes: &[0x48, 0x2d, 0xff, 0xff, 0xff, 0xff],
        lhs: 0,
        rhs: 0,
        rax: 1,
        flags: 0x11,
    },
];

/// mov r9, 41; mov [rsp-8], r9; mov rax, [rsp-8]; add rax, 1.
pub const MOVE_AND_ADD: &[u8] = &[
    0x49, 0xb9, 41, 0, 0, 0, 0, 0, 0, 0, 0x4c, 0x89, 0x4c, 0x24, 0xf8, 0x48, 0x8b, 0x44, 0x24,
    0xf8, 0x48, 0x83, 0xc0, 1,
];

/// call +2; jmp +5; add rax, 1; ret.
pub const CALL_AND_RETURN: &[u8] = &[0xe8, 2, 0, 0, 0, 0xeb, 5, 0x48, 0x83, 0xc0, 1, 0xc3];
