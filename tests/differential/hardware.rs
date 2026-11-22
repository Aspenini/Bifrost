//! Fixed native oracles. All assembly is compiled into the test executable.

use core::arch::asm;

#[derive(Clone, Copy, Debug)]
pub enum Arithmetic {
    Add,
    Sub,
    Cmp,
}

macro_rules! scalar {
    ($opcode:literal, $lhs:expr, $rhs:expr) => {{
        let result: u64;
        let flags: u64;
        // SAFETY: Operands are initialized integers. PUSHFQ and POP balance the stack.
        unsafe {
            asm!(
                concat!($opcode, " rax, {rhs}"),
                "pushfq",
                "pop {flags}",
                inout("rax") $lhs => result,
                rhs = in(reg) $rhs,
                flags = lateout(reg) flags,
            );
        }
        (result, flags)
    }};
}

pub fn arithmetic(op: Arithmetic, lhs: u64, rhs: u64) -> (u64, u64) {
    match op {
        Arithmetic::Add => scalar!("add", lhs, rhs),
        Arithmetic::Sub => scalar!("sub", lhs, rhs),
        Arithmetic::Cmp => scalar!("cmp", lhs, rhs),
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Branch {
    Equal,
    NotEqual,
    Below,
    AboveOrEqual,
    Less,
    GreaterOrEqual,
}

macro_rules! branch {
    ($opcode:literal, $lhs:expr, $rhs:expr) => {{
        let taken: u64;
        // SAFETY: All inputs are integers and branches stay within the assembly block.
        unsafe {
            asm!(
                "cmp {lhs}, {rhs}",
                "mov eax, 0",
                concat!($opcode, " 2f"),
                "jmp 3f",
                "2:",
                "mov eax, 1",
                "3:",
                lhs = in(reg) $lhs,
                rhs = in(reg) $rhs,
                lateout("rax") taken,
                options(nomem, nostack),
            );
        }
        taken == 1
    }};
}

pub fn branch(condition: Branch, lhs: u64, rhs: u64) -> bool {
    match condition {
        Branch::Equal => branch!("je", lhs, rhs),
        Branch::NotEqual => branch!("jne", lhs, rhs),
        Branch::Below => branch!("jb", lhs, rhs),
        Branch::AboveOrEqual => branch!("jae", lhs, rhs),
        Branch::Less => branch!("jl", lhs, rhs),
        Branch::GreaterOrEqual => branch!("jge", lhs, rhs),
    }
}

pub fn move_and_add() -> (u64, [u8; 8], u64) {
    let mut bytes = [0_u8; 9];
    let result: u64;
    let flags: u64;
    // SAFETY: Both memory operands access bytes 1..9 of the uniquely owned local array.
    // PUSHFQ and POP balance the stack. The assembly declares both clobbered registers.
    unsafe {
        asm!(
            "mov r9, 41",
            "mov [{base}+1], r9",
            "mov rax, [{base}+1]",
            "add rax, 1",
            "pushfq",
            "pop {flags}",
            base = in(reg) bytes.as_mut_ptr(),
            out("rax") result,
            out("r9") _,
            flags = lateout(reg) flags,
        );
    }
    (result, bytes[1..].try_into().unwrap(), flags)
}

pub fn indexed_immediate_store() -> (u64, [u8; 8]) {
    let mut bytes = [0_u8; 17];
    let result: u64;
    // SAFETY: Index is 2, so the eight-byte access is inside bytes 9..17 of the array.
    unsafe {
        asm!(
            "mov qword ptr [{base}+{index}*4+1], -2",
            "mov rax, [{base}+{index}*4+1]",
            base = in(reg) bytes.as_mut_ptr(),
            index = in(reg) 2_u64,
            lateout("rax") result,
            options(nostack),
        );
    }
    (result, bytes[9..].try_into().unwrap())
}

pub fn move_and_jump() -> u64 {
    let result: u64;
    // SAFETY: These instructions use declared registers and local labels. The qword is inside
    // the assembly block and is reached only by the RIP-relative load.
    unsafe {
        asm!(
            "mov rax, -2",
            "mov rcx, rax",
            "nop",
            "jmp 3f",
            "2:",
            ".quad 0x0123456789abcdef",
            "3:",
            "mov rax, [rip+2b]",
            "add rax, rcx",
            lateout("rax") result,
            out("rcx") _,
            options(nostack),
        );
    }
    result
}

pub fn call_and_return(initial_rax: u64) -> (u64, u64, bool, bool) {
    let result: u64;
    let flags: u64;
    let start: u64;
    let restored: u64;
    let pushed: u64;
    let expected: u64;
    // SAFETY: CALL targets a label in this block. RET consumes that exact return address.
    // The called code touches only declared registers and the stack slot written by CALL.
    unsafe {
        asm!(
            "mov {start}, rsp",
            "call 2f",
            "3:",
            "jmp 4f",
            "2:",
            "mov {pushed}, [rsp]",
            "add rax, 1",
            "ret",
            "4:",
            "mov {restored}, rsp",
            "lea {expected}, [rip+3b]",
            "pushfq",
            "pop {flags}",
            inout("rax") initial_rax => result,
            start = out(reg) start,
            restored = out(reg) restored,
            pushed = out(reg) pushed,
            expected = out(reg) expected,
            flags = out(reg) flags,
        );
    }
    (result, flags, pushed == expected, start == restored)
}
