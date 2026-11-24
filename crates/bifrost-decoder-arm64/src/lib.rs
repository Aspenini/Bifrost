//! Fixed-width decoder for Bifröst's bare A64 scalar profile.
use bifrost_core::{GuestAddress, GuestMemoryRead, MemoryError};
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Logical {
    And,
    Orr,
    Eor,
    Ands,
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
    LogicalImmediate {
        kind: Logical,
        dst: Reg,
        src: Reg,
        mask: u64,
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
    Return(Reg),
    SupervisorCall {
        immediate: u16,
    },
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
    if let kind @ (0x1280_0000 | 0x5280_0000) = word & 0xffc0_0000 {
        let value = ((word >> 5) & 0xffff) << (((word >> 21) & 1) * 16);
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
        let kind = match (word >> 29) & 3 {
            0 => Logical::And,
            1 => Logical::Orr,
            2 => Logical::Eor,
            _ => Logical::Ands,
        };
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
    if word & 0x1fe0_fc00 == 0x0b00_0000 {
        let rhs = Operand::Register(reg(word >> 16, false));
        return arithmetic(word, dst, lhs, rhs);
    }
    if matches!(word & 0xffc0_0000, 0xf900_0000 | 0xf940_0000) && lhs != Reg::Zero {
        return Operation::Memory {
            reg: dst,
            base: lhs,
            offset: u64::from((word >> 10) & 0xfff) * 8,
            load: word & (1 << 22) != 0,
        };
    }
    if matches!(word & 0xffc0_0000, 0x3900_0000 | 0x3940_0000) && lhs != Reg::Zero {
        return Operation::ByteMemory {
            reg: dst,
            base: lhs,
            offset: u64::from((word >> 10) & 0xfff),
            load: word & (1 << 22) != 0,
        };
    }
    if matches!(word & 0xffe0_0c00, 0x3800_0000 | 0x3840_0000) && lhs != Reg::Zero {
        return Operation::ByteMemory {
            reg: dst,
            base: lhs,
            offset: sign_extend(word >> 12, 9) as u64,
            load: word & (1 << 22) != 0,
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
    if word & 0xfe00_0000 == 0xb400_0000 {
        return Operation::BranchZero {
            target: relative(pc, word >> 5, 19),
            reg: dst,
            nonzero: word & (1 << 24) != 0,
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
