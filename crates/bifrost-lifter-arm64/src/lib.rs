//! Lift the bare ARM64 scalar profile to verified-backend-neutral Bifröst IR.

use bifrost_core::{
    AbiId, CpuFeatureSet, FaultClass, GuestAddress, GuestIsa, GuestMemoryRead, GuestOs,
    GuestTarget, TargetError,
};
use bifrost_decoder_arm64::{DecodeError, DecodedInstruction, Operand, Operation, Reg, fetch};
use bifrost_ir::*;
use std::fmt;

const NZCV: Register = Register::Arm64(Arm64Reg::Nzcv);
const QWORD: MemoryLayout = MemoryLayout {
    ty: Type::I64,
    alignment: 1,
    endian: Endian::Little,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Arm64Lifter;

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LiftError {
    Decode(DecodeError),
    Target(TargetError),
    SupervisorCall {
        pc: GuestAddress,
        immediate: u16,
    },
    UnsupportedInstruction {
        pc: GuestAddress,
        word: u32,
        features: CpuFeatureSet,
    },
}

impl fmt::Display for LiftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => error.fmt(f),
            Self::Target(error) => error.fmt(f),
            Self::SupervisorCall { pc, immediate } => {
                write!(f, "ARM64 supervisor call #{immediate:#06x} at {:#x}", pc.0)
            }
            Self::UnsupportedInstruction { pc, word, features } => write!(
                f,
                "unsupported ARM64 instruction {word:#010x} at {:#x} ({features:?})",
                pc.0
            ),
        }
    }
}

impl std::error::Error for LiftError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            Self::Target(error) => Some(error),
            _ => None,
        }
    }
}

impl InstructionLifter for Arm64Lifter {
    type Error = LiftError;
    fn guest_isa(&self) -> GuestIsa {
        GuestIsa::AArch64
    }
    fn lift_block(
        &self,
        memory: &dyn GuestMemoryRead,
        pc: GuestAddress,
        features: &CpuFeatureSet,
    ) -> Result<IrFunction, LiftError> {
        self.lift(fetch(memory, pc).map_err(LiftError::Decode)?, features)
    }
}

impl Arm64Lifter {
    pub fn lift(
        &self,
        instruction: DecodedInstruction,
        features: &CpuFeatureSet,
    ) -> Result<IrFunction, LiftError> {
        let target = GuestTarget::new(
            GuestIsa::AArch64,
            GuestOs::None,
            AbiId::Bare,
            features.clone(),
        )
        .map_err(LiftError::Target)?;
        let pc = instruction.pc();
        let next = instruction.next_pc();
        let mut ir = Builder {
            pc,
            instructions: Vec::new(),
        };
        let exit = match instruction.operation() {
            Operation::Nop => ir.next(next),
            Operation::Move { dst, src } => {
                let value = ir.operand(src);
                ir.write(dst, value);
                ir.next(next)
            }
            Operation::MoveKeep { dst, bits, shift } => {
                let current = ir.read(dst);
                let kept = ir.with_immediate(IntOp::And, current, !(0xffff_u64 << shift));
                let result = ir.with_immediate(IntOp::Or, kept, u64::from(bits) << shift);
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::Arithmetic {
                dst,
                lhs,
                rhs,
                sub,
                flags,
            } => {
                let lhs = ir.read(lhs);
                let rhs = ir.operand(rhs);
                let result = ir.binary(if sub { IntOp::Sub } else { IntOp::Add }, lhs, rhs);
                if flags {
                    ir.flags(lhs, rhs, result, sub);
                }
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::Memory {
                reg,
                base,
                offset,
                load,
            } => {
                let base = ir.read(base);
                let address = ir.with_immediate(IntOp::Add, base, offset);
                let address = ir.address(address);
                let access = if load {
                    MemoryAccess::Read
                } else {
                    MemoryAccess::Write
                };
                let guard = ir.value(|out| Op::Guard {
                    out,
                    address,
                    access,
                    layout: QWORD,
                });
                if load {
                    let value = ir.value(|out| Op::Load {
                        out,
                        guard,
                        layout: QWORD,
                    });
                    ir.write(reg, value);
                } else {
                    let value = ir.read(reg);
                    ir.emit(Op::Store {
                        guard,
                        value,
                        layout: QWORD,
                    });
                }
                ir.next(next)
            }
            Operation::Branch { target, link } => {
                if link {
                    let lr = ir.integer(next.0);
                    ir.write(Reg::X(30), lr);
                }
                ir.next(target)
            }
            Operation::BranchFlags { target, condition } => {
                let take = ir.condition(condition);
                ir.branch(take, target, next)
            }
            Operation::BranchZero {
                target,
                reg,
                nonzero,
            } => {
                let value = ir.read(reg);
                let zero = ir.integer(0);
                let take = ir.compare(
                    if nonzero {
                        IntPredicate::Ne
                    } else {
                        IntPredicate::Eq
                    },
                    value,
                    zero,
                );
                ir.branch(take, target, next)
            }
            Operation::Return(reg) => {
                let destination = ir.read(reg);
                Exit::NextPc(ir.address(destination))
            }
            Operation::SupervisorCall { immediate } => {
                return Err(LiftError::SupervisorCall { pc, immediate });
            }
            Operation::Fence => {
                ir.emit(Op::Fence);
                ir.next(next)
            }
            Operation::Undefined => Exit::Fault(FaultClass::IllegalInstruction),
            _ => {
                return Err(LiftError::UnsupportedInstruction {
                    pc,
                    word: instruction.word(),
                    features: features.clone(),
                });
            }
        };
        Ok(IrFunction {
            target,
            entry: BlockId(0),
            blocks: vec![IrBlock {
                id: BlockId(0),
                span: CodeSpan { start: pc, len: 4 },
                parameters: vec![],
                instructions: ir.instructions,
                exit_pc: pc,
                exit,
            }],
        })
    }
}

struct Builder {
    pc: GuestAddress,
    instructions: Vec<Instruction>,
}

impl Builder {
    fn emit(&mut self, op: Op) {
        self.instructions.push(Instruction { pc: self.pc, op });
    }

    fn value(&mut self, make: impl FnOnce(ValueId) -> Op) -> ValueId {
        let out = ValueId(self.instructions.len() as u32);
        self.emit(make(out));
        out
    }

    fn constant(&mut self, ty: Type, bits: u64) -> ValueId {
        self.value(|out| Op::Const {
            out,
            ty,
            bits: bits.into(),
        })
    }

    fn integer(&mut self, bits: u64) -> ValueId {
        self.constant(Type::I64, bits)
    }

    fn operand(&mut self, operand: Operand) -> ValueId {
        match operand {
            Operand::Register(reg) => self.read(reg),
            Operand::Immediate(value) => self.integer(value),
        }
    }

    fn read(&mut self, reg: Reg) -> ValueId {
        match reg {
            Reg::Zero => self.integer(0),
            Reg::Sp => {
                let value = self.value(|out| Op::ReadRegister {
                    out,
                    register: Register::Arm64(Arm64Reg::Sp),
                });
                self.value(|out| Op::AddressToInt { out, value })
            }
            Reg::X(index) => self.value(|out| Op::ReadRegister {
                out,
                register: Register::Arm64(Arm64Reg::X(index)),
            }),
        }
    }

    fn write(&mut self, reg: Reg, value: ValueId) {
        let (register, value) = match reg {
            Reg::Zero => return,
            Reg::Sp => (Arm64Reg::Sp, self.address(value)),
            Reg::X(index) => (Arm64Reg::X(index), value),
        };
        self.emit(Op::WriteRegister {
            register: Register::Arm64(register),
            value,
        });
    }

    fn address(&mut self, value: ValueId) -> ValueId {
        self.value(|out| Op::IntToAddress { out, value })
    }

    fn next(&mut self, pc: GuestAddress) -> Exit {
        Exit::NextPc(self.constant(Type::GuestAddress, pc.0))
    }

    fn branch(&mut self, condition: ValueId, target: GuestAddress, next: GuestAddress) -> Exit {
        let yes = self.constant(Type::GuestAddress, target.0);
        let no = self.constant(Type::GuestAddress, next.0);
        Exit::NextPc(self.value(|out| Op::Select {
            out,
            condition,
            yes,
            no,
        }))
    }

    fn binary(&mut self, op: IntOp, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.value(|out| Op::Int { out, op, lhs, rhs })
    }

    fn with_immediate(&mut self, op: IntOp, lhs: ValueId, imm: u64) -> ValueId {
        let rhs = self.integer(imm);
        self.binary(op, lhs, rhs)
    }

    fn compare(&mut self, predicate: IntPredicate, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.value(|out| Op::Compare {
            out,
            predicate,
            lhs,
            rhs,
        })
    }

    fn cast(&mut self, value: ValueId, ty: Type) -> ValueId {
        self.value(|out| Op::IntCast { out, value, ty })
    }

    fn shifted_mask(&mut self, value: ValueId, shift: u64, mask: u64) -> ValueId {
        let count = self.integer(shift);
        let value = self.value(|out| Op::Shift {
            out,
            op: ShiftOp::LogicalRight,
            value,
            count,
            rule: ShiftCount::Mask6,
        });
        self.with_immediate(IntOp::And, value, mask)
    }

    fn mask_if(&mut self, condition: ValueId, mask: u64, no: ValueId) -> ValueId {
        let yes = self.integer(mask);
        self.value(|out| Op::Select {
            out,
            condition,
            yes,
            no,
        })
    }

    fn flags(&mut self, lhs: ValueId, rhs: ValueId, result: ValueId, sub: bool) {
        let zero = self.integer(0);
        let n = self.shifted_mask(result, 32, 1 << 31);
        let is_zero = self.compare(IntPredicate::Eq, result, zero);
        let z = self.mask_if(is_zero, 1 << 30, zero);
        let carry = if sub {
            self.compare(IntPredicate::UnsignedLe, rhs, lhs)
        } else {
            self.compare(IntPredicate::UnsignedLt, result, lhs)
        };
        let c = self.mask_if(carry, 1 << 29, zero);
        let sign_change = self.binary(IntOp::Xor, lhs, result);
        let affected = self.binary(IntOp::Xor, if sub { lhs } else { result }, rhs);
        let overflow = self.binary(IntOp::And, sign_change, affected);
        let v = self.shifted_mask(overflow, 35, 1 << 28);
        let nz = self.binary(IntOp::Or, n, z);
        let cv = self.binary(IntOp::Or, c, v);
        let flags = self.binary(IntOp::Or, nz, cv);
        let value = self.cast(flags, Type::I32);
        self.emit(Op::WriteRegister {
            register: NZCV,
            value,
        });
    }

    fn condition(&mut self, code: u8) -> ValueId {
        if code >= 14 {
            return self.constant(Type::I1, 1);
        }
        let flags = self.value(|out| Op::ReadRegister {
            out,
            register: NZCV,
        });
        let flags = self.cast(flags, Type::I64);
        let zero = self.integer(0);
        let bit = |ir: &mut Self, mask| {
            let value = ir.with_immediate(IntOp::And, flags, mask);
            ir.compare(IntPredicate::Ne, value, zero)
        };
        let result = match code >> 1 {
            0 => bit(self, 1 << 30), // EQ
            1 => bit(self, 1 << 29), // CS
            2 => bit(self, 1 << 31), // MI
            3 => bit(self, 1 << 28), // VS
            4 => {
                // HI
                let c = bit(self, 1 << 29);
                let z = bit(self, 1 << 30);
                let clear_z = self.invert(z);
                self.binary(IntOp::And, c, clear_z)
            }
            5 | 6 => {
                // GE, GT
                let n = bit(self, 1 << 31);
                let v = bit(self, 1 << 28);
                let equal = self.compare(IntPredicate::Eq, n, v);
                if code >> 1 == 5 {
                    equal
                } else {
                    let z = bit(self, 1 << 30);
                    let clear_z = self.invert(z);
                    self.binary(IntOp::And, equal, clear_z)
                }
            }
            _ => unreachable!("decoded four-bit condition"),
        };
        if code & 1 == 0 {
            result
        } else {
            self.invert(result)
        }
    }

    fn invert(&mut self, value: ValueId) -> ValueId {
        let one = self.constant(Type::I1, 1);
        self.binary(IntOp::Xor, value, one)
    }
}
