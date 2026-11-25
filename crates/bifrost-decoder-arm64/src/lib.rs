//! Fixed-width decoder for Bifröst's bare A64 scalar profile.
use bifrost_core::{
    Arm64PointerAction, Arm64PointerAuth, Arm64PointerKey, Arm64PointerModifier, GuestAddress,
    GuestMemoryRead, MemoryError,
};
use std::fmt;

pub const INSTRUCTION_BYTES: u64 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reg {
    X(u8),
    Sp,
    Zero,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operand {
    Register(Reg),
    Immediate(u64),
    Extended {
        reg: Reg,
        bits: u8,
        signed: bool,
        shift: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Logical {
    And,
    Orr,
    Eor,
    Ands,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterShift {
    Left,
    LogicalRight,
    ArithmeticRight,
    RotateRight,
}

impl RegisterShift {
    fn decode(bits: u32) -> Self {
        match bits & 3 {
            0 => Self::Left,
            1 => Self::LogicalRight,
            2 => Self::ArithmeticRight,
            _ => Self::RotateRight,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Bitfield {
    Signed,
    Merge,
    Unsigned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConditionalElse {
    Keep,
    Increment,
    Invert,
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressExtend {
    UnsignedWord,
    SignedWord,
    Doubleword,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairAddressing {
    Offset,
    PostIndex,
    PreIndex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Operation {
    Nop,
    Move {
        dst: Reg,
        src: Operand,
    },
    MoveWord {
        dst: Reg,
        src: Reg,
    },
    MoveKeep {
        dst: Reg,
        bits: u16,
        shift: u8,
    },
    MoveKeepWord {
        dst: Reg,
        bits: u16,
        shift: u8,
    },
    Arithmetic {
        dst: Reg,
        lhs: Reg,
        rhs: Operand,
        sub: bool,
        flags: bool,
    },
    ArithmeticWord {
        dst: Reg,
        lhs: Reg,
        rhs: Operand,
        sub: bool,
        flags: bool,
    },
    ArithmeticShifted {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        shift: RegisterShift,
        amount: u8,
        sub: bool,
        flags: bool,
        word: bool,
    },
    MultiplyAccumulate {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        accumulator: Reg,
        sub: bool,
        word: bool,
        extend: AddressExtend,
    },
    MultiplyHigh {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        signed: bool,
    },
    Divide {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        signed: bool,
        word: bool,
    },
    LogicalImmediate {
        kind: Logical,
        dst: Reg,
        src: Reg,
        mask: u64,
        word: bool,
    },
    LogicalRegister {
        kind: Logical,
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        shift: RegisterShift,
        amount: u8,
        invert: bool,
        word: bool,
    },
    Bitfield {
        kind: Bitfield,
        dst: Reg,
        src: Reg,
        src_start: u8,
        dst_start: u8,
        len: u8,
        word: bool,
    },
    ReverseBits {
        dst: Reg,
        src: Reg,
        word: bool,
    },
    CountLeadingBits {
        dst: Reg,
        src: Reg,
        word: bool,
        sign: bool,
    },
    VariableShift {
        dst: Reg,
        src: Reg,
        count: Reg,
        shift: RegisterShift,
        word: bool,
    },
    Memory {
        reg: Reg,
        base: Reg,
        offset: u64,
        load: bool,
    },
    ByteMemory {
        reg: Reg,
        base: Reg,
        offset: u64,
        load: bool,
    },
    HalfwordMemory {
        reg: Reg,
        base: Reg,
        offset: u64,
        load: bool,
    },
    WordMemory {
        reg: Reg,
        base: Reg,
        offset: u64,
        load: bool,
    },
    IndexedMemory {
        reg: Reg,
        base: Reg,
        offset: u64,
        load: bool,
        size_log2: u8,
        pre_index: bool,
    },
    RegisterOffsetMemory {
        reg: Reg,
        base: Reg,
        index: Reg,
        extend: AddressExtend,
        scale: bool,
        size_log2: u8,
        load: bool,
    },
    WordPair {
        first: Reg,
        second: Reg,
        base: Reg,
        offset: u64,
        load: bool,
        addressing: PairAddressing,
    },
    DoublePair {
        first: Reg,
        second: Reg,
        base: Reg,
        offset: u64,
        load: bool,
        addressing: PairAddressing,
    },
    Branch {
        target: GuestAddress,
        link: bool,
    },
    BranchFlags {
        target: GuestAddress,
        condition: u8,
    },
    BranchZero {
        target: GuestAddress,
        reg: Reg,
        nonzero: bool,
    },
    BranchZeroWord {
        target: GuestAddress,
        reg: Reg,
        nonzero: bool,
    },
    BranchBit {
        target: GuestAddress,
        reg: Reg,
        bit: u8,
        nonzero: bool,
    },
    ConditionalSelect {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        condition: u8,
        otherwise: ConditionalElse,
        word: bool,
    },
    ConditionalCompare {
        lhs: Reg,
        rhs: Operand,
        condition: u8,
        fallback: u8,
        sub: bool,
        word: bool,
    },
    Return(Reg),
    BranchRegister {
        reg: Reg,
        link: bool,
    },
    SupervisorCall {
        immediate: u16,
    },
    PointerAuth(Arm64PointerAuth),
    Fence,
    Undefined,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedInstruction {
    pc: GuestAddress,
    word: u32,
    operation: Operation,
}

impl DecodedInstruction {
    pub const fn pc(self) -> GuestAddress {
        self.pc
    }
    pub const fn word(self) -> u32 {
        self.word
    }
    pub const fn operation(self) -> Operation {
        self.operation
    }
    pub const fn bytes(self) -> [u8; 4] {
        self.word.to_le_bytes()
    }
    pub const fn next_pc(self) -> GuestAddress {
        GuestAddress(self.pc.0.wrapping_add(INSTRUCTION_BYTES))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DecodeError {
    Alignment(GuestAddress),
    Fetch {
        pc: GuestAddress,
        source: MemoryError,
    },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Alignment(pc) => write!(f, "unaligned ARM64 instruction fetch at {:#x}", pc.0),
            Self::Fetch { pc, source } => {
                write!(f, "ARM64 instruction fetch at {:#x}: {source}", pc.0)
            }
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Fetch { source, .. } => Some(source),
            Self::Alignment(_) => None,
        }
    }
}

pub fn fetch(
    memory: &dyn GuestMemoryRead,
    pc: GuestAddress,
) -> Result<DecodedInstruction, DecodeError> {
    check_pc(pc)?;
    let mut bytes = [0; 4];
    memory
        .fetch(pc, &mut bytes)
        .map_err(|source| DecodeError::Fetch { pc, source })?;
    decode(bytes, pc)
}

pub fn decode(bytes: [u8; 4], pc: GuestAddress) -> Result<DecodedInstruction, DecodeError> {
    check_pc(pc)?;
    let word = u32::from_le_bytes(bytes);
    Ok(DecodedInstruction {
        pc,
        word,
        operation: operation(word, pc),
    })
}

fn check_pc(pc: GuestAddress) -> Result<(), DecodeError> {
    if pc.0.is_multiple_of(INSTRUCTION_BYTES) {
        Ok(())
    } else {
        Err(DecodeError::Alignment(pc))
    }
}

fn reg(bits: u32, sp: bool) -> Reg {
    match (bits & 31, sp) {
        (31, true) => Reg::Sp,
        (31, false) => Reg::Zero,
        (n, _) => Reg::X(n as u8),
    }
}

fn sign_extend(bits: u32, width: u32) -> i64 {
    let shift = 32 - width;
    i64::from(((bits << shift) as i32) >> shift)
}

fn relative(pc: GuestAddress, bits: u32, width: u32) -> GuestAddress {
    GuestAddress(pc.0.wrapping_add_signed(sign_extend(bits, width) * 4))
}

fn arithmetic(word: u32, dst: Reg, lhs: Reg, rhs: Operand) -> Operation {
    let sub = word & (1 << 30) != 0;
    let flags = word & (1 << 29) != 0;
    if word >> 31 == 0 {
        Operation::ArithmeticWord {
            dst,
            lhs,
            rhs,
            sub,
            flags,
        }
    } else {
        Operation::Arithmetic {
            dst,
            lhs,
            rhs,
            sub,
            flags,
        }
    }
}

fn logical_immediate_mask(instruction: u32, bits: u32) -> Option<u64> {
    let n = (instruction >> 22) & 1;
    let immr = (instruction >> 16) & 0x3f;
    let imms = (instruction >> 10) & 0x3f;
    // The first set bit of N:~imms determines the width of a repeated element.
    let len = ((n << 6) | (!imms & 0x3f)).checked_ilog2()?;
    let width = 1 << len;
    if width < 2 || width > bits {
        return None;
    }
    let ones = (imms & (width - 1)) + 1;
    if ones == width {
        return None;
    }
    let rotation = immr & (width - 1);
    let mut element = (1_u64 << ones) - 1;
    if rotation != 0 {
        element = (element >> rotation) | (element << (width - rotation));
        element &= u64::MAX >> (64 - width);
    }
    let mut mask = 0;
    for offset in (0..bits).step_by(width as usize) {
        mask |= element << offset;
    }
    Some(mask)
}

fn logical_kind(word: u32) -> Logical {
    match (word >> 29) & 3 {
        0 => Logical::And,
        1 => Logical::Orr,
        2 => Logical::Eor,
        _ => Logical::Ands,
    }
}

fn pointer_auth(word: u32) -> Option<Arm64PointerAuth> {
    use Arm64PointerAction::{Authenticate, AuthenticateReturn, Sign};
    use Arm64PointerKey::{DataA, DataB, InstructionA, InstructionB};
    use Arm64PointerModifier::{Register, StackPointer, Zero};

    let (action, key) = match word {
        0xd503_233f => (Sign, InstructionA),
        0xd503_237f => (Sign, InstructionB),
        0xd503_23bf => (Authenticate, InstructionA),
        0xd503_23ff => (Authenticate, InstructionB),
        0xd65f_0bff => (AuthenticateReturn, InstructionA),
        0xd65f_0fff => (AuthenticateReturn, InstructionB),
        _ if word & 0xffff_c000 == 0xdac1_0000 => {
            let pointer_register = (word & 31) as u8;
            let modifier_reg = ((word >> 5) & 31) as u8;
            let modifier = if word & (1 << 13) != 0 {
                if modifier_reg != 31 {
                    return None;
                }
                Zero
            } else if modifier_reg == 31 {
                StackPointer
            } else {
                Register(modifier_reg)
            };
            let action = if word & (1 << 12) == 0 {
                Sign
            } else {
                Authenticate
            };
            let key = [InstructionA, InstructionB, DataA, DataB][((word >> 10) & 3) as usize];
            return Some(Arm64PointerAuth {
                action,
                key,
                pointer_register,
                modifier,
            });
        }
        _ => return None,
    };
    Some(Arm64PointerAuth {
        action,
        key,
        pointer_register: 30,
        modifier: StackPointer,
    })
}

fn operation(word: u32, pc: GuestAddress) -> Operation {
    let dst = reg(word, false);
    let lhs = reg(word >> 5, false);
    match word {
        0xd503_201f => return Operation::Nop,
        0xd503_3bbf => return Operation::Fence, // DMB ISH
        _ => {}
    }
    if word & 0xffe0_001f == 0xd400_0001 {
        return Operation::SupervisorCall {
            immediate: ((word >> 5) & 0xffff) as u16,
        };
    }
    if let Some(operation) = pointer_auth(word) {
        return Operation::PointerAuth(operation);
    }
    if word & 0x7fff_fc00 == 0x5ac0_0000 {
        return Operation::ReverseBits {
            dst,
            src: lhs,
            word: word >> 31 == 0,
        };
    }
    if matches!(word & 0x7fff_fc00, 0x5ac0_1000 | 0x5ac0_1400) {
        return Operation::CountLeadingBits {
            dst,
            src: lhs,
            word: word >> 31 == 0,
            sign: word & (1 << 10) != 0,
        };
    }
    if word & 0x1f00_0000 == 0x1000_0000 {
        let imm = ((word >> 5) & 0x7ffff) << 2 | ((word >> 29) & 3);
        let displacement = sign_extend(imm, 21);
        let address = if word >> 31 == 0 {
            pc.0.wrapping_add_signed(displacement)
        } else {
            (pc.0 & !0xfff).wrapping_add_signed(displacement << 12)
        };
        return Operation::Move {
            dst,
            src: Operand::Immediate(address),
        };
    }
    if let kind @ (0x1280_0000 | 0x5280_0000 | 0x7280_0000) = word & 0xffc0_0000 {
        let bits = ((word >> 5) & 0xffff) as u16;
        let shift = (((word >> 21) & 1) * 16) as u8;
        if kind == 0x7280_0000 {
            return Operation::MoveKeepWord { dst, bits, shift };
        }
        let value = u32::from(bits) << shift;
        let value = if kind == 0x1280_0000 { !value } else { value };
        return Operation::Move {
            dst,
            src: Operand::Immediate(u64::from(value)),
        };
    }
    if let kind @ (0x9280_0000 | 0xd280_0000 | 0xf280_0000) = word & 0xff80_0000 {
        let bits = ((word >> 5) & 0xffff) as u16;
        let shift = ((word >> 21) & 3) as u8 * 16;
        let value = u64::from(bits) << shift;
        return match kind {
            0x9280_0000 => Operation::Move {
                dst,
                src: Operand::Immediate(!value),
            },
            0xd280_0000 => Operation::Move {
                dst,
                src: Operand::Immediate(value),
            },
            _ => Operation::MoveKeep { dst, bits, shift },
        };
    }
    match word & 0xffe0_ffe0 {
        0x2a00_03e0 => {
            return Operation::MoveWord {
                dst,
                src: reg(word >> 16, false),
            };
        }
        0xaa00_03e0 => {
            return Operation::Move {
                dst,
                src: Operand::Register(reg(word >> 16, false)),
            };
        }
        _ => {}
    }
    if word & 0x1f00_0000 == 0x0a00_0000 {
        let narrow = word >> 31 == 0;
        let amount = ((word >> 10) & 63) as u8;
        if narrow && amount >= 32 {
            return Operation::Undefined;
        }
        let shift = RegisterShift::decode(word >> 22);
        return Operation::LogicalRegister {
            kind: logical_kind(word),
            dst,
            lhs,
            rhs: reg(word >> 16, false),
            shift,
            amount,
            invert: word & (1 << 21) != 0,
            word: narrow,
        };
    }
    if word & 0x1f80_0000 == 0x1100_0000 {
        let shift = if word & (1 << 22) == 0 { 0 } else { 12 };
        let rhs = Operand::Immediate(u64::from((word >> 10) & 0xfff) << shift);
        return arithmetic(
            word,
            reg(word, word & (1 << 29) == 0),
            reg(word >> 5, true),
            rhs,
        );
    }
    if word & 0x1f80_0000 == 0x1200_0000 {
        let narrow = word >> 31 == 0;
        let kind = logical_kind(word);
        let Some(mask) = logical_immediate_mask(word, if narrow { 32 } else { 64 }) else {
            return Operation::Undefined;
        };
        return Operation::LogicalImmediate {
            kind,
            dst: reg(word, kind != Logical::Ands),
            src: lhs,
            mask,
            word: narrow,
        };
    }
    if word & 0x1f80_0000 == 0x1300_0000 {
        let narrow = word >> 31 == 0;
        let width = if narrow { 32 } else { 64 };
        let r = ((word >> 16) & 63) as u8;
        let s = ((word >> 10) & 63) as u8;
        if (word & (1 << 22) == 0) != narrow || r >= width || s >= width {
            return Operation::Undefined;
        }
        let kind = match (word >> 29) & 3 {
            0 => Bitfield::Signed,
            1 => Bitfield::Merge,
            2 => Bitfield::Unsigned,
            _ => return Operation::Undefined,
        };
        let (src_start, dst_start, len) = if s < r {
            (0, width - r, s + 1)
        } else {
            (r, 0, s - r + 1)
        };
        return Operation::Bitfield {
            kind,
            dst,
            src: lhs,
            src_start,
            dst_start,
            len,
            word: narrow,
        };
    }
    if word & 0x1fe0_0000 == 0x0b20_0000 {
        let shift = ((word >> 10) & 7) as u8;
        if shift > 4 {
            return Operation::Undefined;
        }
        let option = (word >> 13) & 7;
        return arithmetic(
            word,
            reg(word, word & (1 << 29) == 0),
            reg(word >> 5, true),
            Operand::Extended {
                reg: reg(word >> 16, false),
                bits: 8 << (option & 3),
                signed: option & 4 != 0,
                shift,
            },
        );
    }
    if word & 0x1f20_0000 == 0x0b00_0000 {
        let rhs = reg(word >> 16, false);
        let shift = RegisterShift::decode(word >> 22);
        let amount = ((word >> 10) & 63) as u8;
        let narrow = word >> 31 == 0;
        if shift == RegisterShift::RotateRight || (narrow && amount >= 32) {
            return Operation::Undefined;
        }
        if amount == 0 {
            return arithmetic(word, dst, lhs, Operand::Register(rhs));
        }
        return Operation::ArithmeticShifted {
            dst,
            lhs,
            rhs,
            shift,
            amount,
            sub: word & (1 << 30) != 0,
            flags: word & (1 << 29) != 0,
            word: narrow,
        };
    }
    if word & 0x3fe0_0410 == 0x3a40_0000 {
        let rhs = if word & (1 << 11) == 0 {
            Operand::Register(reg(word >> 16, false))
        } else {
            Operand::Immediate(u64::from((word >> 16) & 31))
        };
        return Operation::ConditionalCompare {
            lhs,
            rhs,
            condition: ((word >> 12) & 15) as u8,
            fallback: (word & 15) as u8,
            sub: word & (1 << 30) != 0,
            word: word >> 31 == 0,
        };
    }
    if word & 0x3fe0_0800 == 0x1a80_0000 {
        let otherwise = match (word & (1 << 30) != 0, word & (1 << 10) != 0) {
            (false, false) => ConditionalElse::Keep,
            (false, true) => ConditionalElse::Increment,
            (true, false) => ConditionalElse::Invert,
            (true, true) => ConditionalElse::Negate,
        };
        return Operation::ConditionalSelect {
            dst,
            lhs,
            rhs: reg(word >> 16, false),
            condition: ((word >> 12) & 15) as u8,
            otherwise,
            word: word >> 31 == 0,
        };
    }
    if word & 0x7fe0_f800 == 0x1ac0_0800 {
        return Operation::Divide {
            dst,
            lhs,
            rhs: reg(word >> 16, false),
            signed: word & (1 << 10) != 0,
            word: word >> 31 == 0,
        };
    }
    if word & 0x7fe0_f000 == 0x1ac0_2000 {
        return Operation::VariableShift {
            dst,
            src: lhs,
            count: reg(word >> 16, false),
            shift: RegisterShift::decode(word >> 10),
            word: word >> 31 == 0,
        };
    }
    if let kind @ (0x9b40_7c00 | 0x9bc0_7c00) = word & 0xffe0_fc00 {
        return Operation::MultiplyHigh {
            dst,
            lhs,
            rhs: reg(word >> 16, false),
            signed: kind == 0x9b40_7c00,
        };
    }
    let multiply_extend = match word & 0xffe0_0000 {
        0x1b00_0000 | 0x9b00_0000 => Some(AddressExtend::Doubleword),
        0x9ba0_0000 => Some(AddressExtend::UnsignedWord),
        0x9b20_0000 => Some(AddressExtend::SignedWord),
        _ => None,
    };
    if let Some(extend) = multiply_extend {
        return Operation::MultiplyAccumulate {
            dst,
            lhs,
            rhs: reg(word >> 16, false),
            accumulator: reg(word >> 10, false),
            sub: word & (1 << 15) != 0,
            word: word >> 31 == 0,
            extend,
        };
    }
    let base = reg(word >> 5, true);
    if word & 0x3fa0_0c00 == 0x3820_0800 {
        let extend = match (word >> 13) & 7 {
            2 => AddressExtend::UnsignedWord,
            6 => AddressExtend::SignedWord,
            3 | 7 => AddressExtend::Doubleword,
            _ => return Operation::Undefined,
        };
        return Operation::RegisterOffsetMemory {
            reg: dst,
            base,
            index: reg(word >> 16, false),
            extend,
            scale: word & (1 << 12) != 0,
            size_log2: (word >> 30) as u8,
            load: word & (1 << 22) != 0,
        };
    }
    let mode = (word >> 10) & 3;
    if matches!(mode, 1 | 3) && word & 0x3fa0_0000 == 0x3800_0000 {
        if dst == base {
            return Operation::Unsupported;
        }
        return Operation::IndexedMemory {
            reg: dst,
            base,
            offset: sign_extend(word >> 12, 9) as u64,
            load: word & (1 << 22) != 0,
            size_log2: (word >> 30) as u8,
            pre_index: mode == 3,
        };
    }
    let size_log2 = word >> 30;
    let offset = if word & 0x3f80_0000 == 0x3900_0000 {
        Some(u64::from((word >> 10) & 0xfff) << size_log2)
    } else if word & 0x3fa0_0c00 == 0x3800_0000 {
        Some(sign_extend(word >> 12, 9) as u64)
    } else {
        None
    };
    if let Some(offset) = offset {
        let load = word & (1 << 22) != 0;
        return match size_log2 {
            0 => Operation::ByteMemory {
                reg: dst,
                base,
                offset,
                load,
            },
            1 => Operation::HalfwordMemory {
                reg: dst,
                base,
                offset,
                load,
            },
            2 => Operation::WordMemory {
                reg: dst,
                base,
                offset,
                load,
            },
            _ => Operation::Memory {
                reg: dst,
                base,
                offset,
                load,
            },
        };
    }
    if word & 0x7e00_0000 == 0x2800_0000 {
        let addressing = match (word >> 23) & 3 {
            1 => PairAddressing::PostIndex,
            2 => PairAddressing::Offset,
            3 => PairAddressing::PreIndex,
            _ => return Operation::Unsupported,
        };
        let load = word & (1 << 22) != 0;
        let second = reg(word >> 10, false);
        if (load && dst == second)
            || (addressing != PairAddressing::Offset && (base == dst || base == second))
        {
            return Operation::Unsupported;
        }
        let imm = sign_extend(word >> 15, 7);
        return if word >> 31 == 0 {
            Operation::WordPair {
                first: dst,
                second,
                base,
                offset: (imm * 4) as u64,
                load,
                addressing,
            }
        } else {
            Operation::DoublePair {
                first: dst,
                second,
                base,
                offset: (imm * 8) as u64,
                load,
                addressing,
            }
        };
    }
    if word & 0x7c00_0000 == 0x1400_0000 {
        return Operation::Branch {
            target: relative(pc, word, 26),
            link: word >> 31 != 0,
        };
    }
    if word & 0xff00_0010 == 0x5400_0000 {
        return Operation::BranchFlags {
            target: relative(pc, word >> 5, 19),
            condition: (word & 15) as u8,
        };
    }
    if word & 0x7e00_0000 == 0x3400_0000 {
        let target = relative(pc, word >> 5, 19);
        let nonzero = word & (1 << 24) != 0;
        return if word >> 31 == 0 {
            Operation::BranchZeroWord {
                target,
                reg: dst,
                nonzero,
            }
        } else {
            Operation::BranchZero {
                target,
                reg: dst,
                nonzero,
            }
        };
    }
    if word & 0x7e00_0000 == 0x3600_0000 {
        return Operation::BranchBit {
            target: relative(pc, word >> 5, 14),
            reg: dst,
            bit: (((word >> 31) << 5) | ((word >> 19) & 31)) as u8,
            nonzero: word & (1 << 24) != 0,
        };
    }
    if let kind @ (0xd61f_0000 | 0xd63f_0000) = word & 0xffff_fc1f {
        return Operation::BranchRegister {
            reg: lhs,
            link: kind == 0xd63f_0000,
        };
    }
    if word & 0xffff_fc1f == 0xd65f_0000 && lhs != Reg::Zero {
        return Operation::Return(lhs);
    }
    if word & 0xffff_0000 == 0 {
        return Operation::Undefined;
    } // UDF
    Operation::Unsupported
}
