//! x86-64 instruction decoding and normalized operands for the initial execution profile.

use bifrost_core::{GuestAddress, GuestMemoryRead, MemoryError};
use iced_x86::{Code, Decoder, DecoderError, DecoderOptions, Instruction, OpKind, Register};
use std::fmt;

pub const MAX_INSTRUCTION_BYTES: usize = 15;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Condition {
    Equal,
    NotEqual,
    Below,
    AboveOrEqual,
    Less,
    GreaterOrEqual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arithmetic {
    Add,
    Sub,
    Compare,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryBase {
    None,
    Register(u8),
    Rip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryOperand {
    pub base: MemoryBase,
    pub index: Option<(u8, u32)>,
    pub displacement: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operand {
    Register(u8),
    Immediate(u64),
    Memory(MemoryOperand),
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Operation {
    Nop,
    Mov {
        dst: Operand,
        src: Operand,
    },
    Arithmetic {
        op: Arithmetic,
        dst: u8,
        src: Operand,
    },
    Branch {
        condition: Option<Condition>,
        target: GuestAddress,
    },
    Call(GuestAddress),
    CallIndirect(Operand),
    Return,
    Ud2,
    Unsupported {
        opcode: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedInstruction {
    pc: GuestAddress,
    bytes: [u8; MAX_INSTRUCTION_BYTES],
    len: u8,
    operation: Operation,
}

impl DecodedInstruction {
    pub const fn pc(&self) -> GuestAddress {
        self.pc
    }

    pub const fn next_pc(&self) -> GuestAddress {
        GuestAddress(self.pc.0.wrapping_add(self.len as u64))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
    pub const fn operation(&self) -> &Operation {
        &self.operation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DecodeError {
    Invalid {
        pc: GuestAddress,
    },
    Truncated {
        pc: GuestAddress,
    },
    NonCanonical {
        pc: GuestAddress,
        address: GuestAddress,
    },
    Fetch {
        pc: GuestAddress,
        source: MemoryError,
    },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { pc } => write!(f, "invalid x86-64 encoding at {:#x}", pc.0),
            Self::Truncated { pc } => write!(f, "truncated x86-64 instruction at {:#x}", pc.0),
            Self::NonCanonical { pc, address } => write!(
                f,
                "x86-64 fetch at {:#x} reached noncanonical address {:#x}",
                pc.0, address.0
            ),
            Self::Fetch { pc, source } => {
                write!(f, "x86-64 instruction fetch at {:#x}: {source}", pc.0)
            }
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Fetch { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Decodes the first instruction of a byte slice, including valid opcodes outside the execution subset.
pub fn decode(bytes: &[u8], pc: GuestAddress) -> Result<DecodedInstruction, DecodeError> {
    let limit = bytes.len().min(MAX_INSTRUCTION_BYTES);
    let mut decoder = Decoder::with_ip(64, &bytes[..limit], pc.0, DecoderOptions::NONE);
    let instruction = decoder.decode();
    if decoder.last_error() == DecoderError::NoMoreBytes && limit < MAX_INSTRUCTION_BYTES {
        return Err(DecodeError::Truncated { pc });
    }
    if instruction.is_invalid() {
        return Err(DecodeError::Invalid { pc });
    }

    let bytes = &bytes[..instruction.len()];
    let mut encoding = [0; MAX_INSTRUCTION_BYTES];
    encoding[..bytes.len()].copy_from_slice(bytes);
    let operation = classify(&instruction, bytes).unwrap_or_else(|| Operation::Unsupported {
        opcode: format!("{:?}", instruction.code()),
    });
    Ok(DecodedInstruction {
        pc,
        bytes: encoding,
        len: bytes.len() as u8,
        operation,
    })
}

/// Fetches only the bytes required to decode an instruction, including across page boundaries.
pub fn fetch(
    memory: &dyn GuestMemoryRead,
    pc: GuestAddress,
) -> Result<DecodedInstruction, DecodeError> {
    let mut bytes = [0; MAX_INSTRUCTION_BYTES];
    for offset in 0..MAX_INSTRUCTION_BYTES {
        let address = pc.checked_add(offset as u64).ok_or(DecodeError::Fetch {
            pc,
            source: MemoryError::AddressOverflow(pc),
        })?;
        if !address.is_canonical_48() {
            return Err(DecodeError::NonCanonical { pc, address });
        }
        memory
            .fetch(address, &mut bytes[offset..=offset])
            .map_err(|source| DecodeError::Fetch { pc, source })?;
        match decode(&bytes[..=offset], pc) {
            Err(DecodeError::Truncated { .. }) => {}
            result => return result,
        }
    }
    Err(DecodeError::Invalid { pc })
}

fn classify(inst: &Instruction, bytes: &[u8]) -> Option<Operation> {
    use Code::*;
    // The decoder retains the length of prefixed instructions even when this profile rejects them.
    let first_opcode = bytes
        .iter()
        .copied()
        .find(|byte| !(0x40..=0x4f).contains(byte))?;
    if matches!(
        first_opcode,
        0xf0 | 0xf2 | 0xf3 | 0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 | 0x66 | 0x67
    ) {
        return None;
    }
    let branch = |condition| Operation::Branch {
        condition,
        target: GuestAddress(inst.near_branch64()),
    };
    let arithmetic = |op| {
        let Operand::Register(dst) = operand(inst, 0)? else {
            return None;
        };
        let src = operand(inst, 1)?;
        if matches!(src, Operand::Memory(_)) {
            return None;
        }
        Some(Operation::Arithmetic { op, dst, src })
    };
    Some(match inst.code() {
        Nopd | Nopq => Operation::Nop,
        Mov_rm64_r64 | Mov_r64_rm64 | Mov_r64_imm64 | Mov_rm64_imm32 => Operation::Mov {
            dst: operand(inst, 0)?,
            src: operand(inst, 1)?,
        },
        Add_rm64_r64 | Add_r64_rm64 | Add_RAX_imm32 | Add_rm64_imm8 | Add_rm64_imm32 => {
            return arithmetic(Arithmetic::Add);
        }
        Sub_rm64_r64 | Sub_r64_rm64 | Sub_RAX_imm32 | Sub_rm64_imm8 | Sub_rm64_imm32 => {
            return arithmetic(Arithmetic::Sub);
        }
        Cmp_rm64_r64 | Cmp_r64_rm64 | Cmp_RAX_imm32 | Cmp_rm64_imm8 | Cmp_rm64_imm32 => {
            return arithmetic(Arithmetic::Compare);
        }
        Jmp_rel8_64 | Jmp_rel32_64 => branch(None),
        Je_rel8_64 | Je_rel32_64 => branch(Some(Condition::Equal)),
        Jne_rel8_64 | Jne_rel32_64 => branch(Some(Condition::NotEqual)),
        Jb_rel8_64 | Jb_rel32_64 => branch(Some(Condition::Below)),
        Jae_rel8_64 | Jae_rel32_64 => branch(Some(Condition::AboveOrEqual)),
        Jl_rel8_64 | Jl_rel32_64 => branch(Some(Condition::Less)),
        Jge_rel8_64 | Jge_rel32_64 => branch(Some(Condition::GreaterOrEqual)),
        Call_rel32_64 => Operation::Call(GuestAddress(inst.near_branch64())),
        Call_rm64 => Operation::CallIndirect(operand(inst, 0)?),
        Retnq => Operation::Return,
        Ud2 => Operation::Ud2,
        _ => return None,
    })
}

fn operand(inst: &Instruction, index: u32) -> Option<Operand> {
    match inst.op_kind(index) {
        OpKind::Register => gpr(inst.op_register(index)).map(Operand::Register),
        OpKind::Immediate8to64 | OpKind::Immediate32to64 | OpKind::Immediate64 => {
            Some(Operand::Immediate(inst.immediate(index)))
        }
        OpKind::Memory => {
            let base = match inst.memory_base() {
                Register::None => MemoryBase::None,
                Register::RIP => MemoryBase::Rip,
                reg => MemoryBase::Register(gpr(reg)?),
            };
            let index = match inst.memory_index() {
                Register::None => None,
                reg => Some((gpr(reg)?, inst.memory_index_scale())),
            };
            let displacement = if base == MemoryBase::Rip {
                inst.ip_rel_memory_address().wrapping_sub(inst.next_ip())
            } else {
                inst.memory_displacement64()
            };
            Some(Operand::Memory(MemoryOperand {
                base,
                index,
                displacement,
            }))
        }
        _ => None,
    }
}

fn gpr(register: Register) -> Option<u8> {
    let index = (register as u32).wrapping_sub(Register::RAX as u32);
    (index < 16).then_some(index as u8)
}
