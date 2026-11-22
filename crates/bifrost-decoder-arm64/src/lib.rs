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
#[non_exhaustive]
pub enum Operation {
    Nop,
    Move {
        dst: Reg,
        src: Operand,
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
    Memory {
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

fn relative(pc: GuestAddress, bits: u32, width: u32) -> GuestAddress {
    let shift = 32 - width;
    let signed = ((bits << shift) as i32) >> shift;
    GuestAddress(pc.0.wrapping_add_signed(i64::from(signed) * 4))
}

fn operation(word: u32, pc: GuestAddress) -> Operation {
    let sub = word & (1 << 30) != 0;
    let flags = word & (1 << 29) != 0;
    let dst = reg(word, false);
    let lhs = reg(word >> 5, false);
    match word {
        0xd503_201f => return Operation::Nop,
        0xd503_3bbf => return Operation::Fence, // DMB ISH
        _ => {}
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
    if word & 0xffe0_ffe0 == 0xaa00_03e0 {
        return Operation::Move {
            dst,
            src: Operand::Register(reg(word >> 16, false)),
        };
    }
    if word & 0x9f80_0000 == 0x9100_0000 {
        let shift = if word & (1 << 22) == 0 { 0 } else { 12 };
        let rhs = Operand::Immediate(u64::from((word >> 10) & 0xfff) << shift);
        return Operation::Arithmetic {
            dst: reg(word, !flags),
            lhs: reg(word >> 5, true),
            rhs,
            sub,
            flags,
        };
    }
    if word & 0x9fe0_fc00 == 0x8b00_0000 {
        let rhs = Operand::Register(reg(word >> 16, false));
        return Operation::Arithmetic {
            dst,
            lhs,
            rhs,
            sub,
            flags,
        };
    }
    if matches!(word & 0xffc0_0000, 0xf900_0000 | 0xf940_0000) && lhs != Reg::Zero {
        return Operation::Memory {
            reg: dst,
            base: lhs,
            offset: u64::from((word >> 10) & 0xfff) * 8,
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
