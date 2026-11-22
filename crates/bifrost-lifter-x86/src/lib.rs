//! Translation from the declared x86-64 subset into architecture-neutral Bifröst IR.

use bifrost_core::{
    AbiId, CpuFeatureSet, FaultClass, GuestAddress, GuestIsa, GuestMemoryRead, GuestOs,
    GuestTarget, TargetError,
};
use bifrost_decoder_x86::{
    Arithmetic, Condition, DecodeError, DecodedInstruction, MemoryBase, MemoryOperand, Operand,
    Operation, fetch,
};
use bifrost_ir::*;
use std::fmt;

/// CF, PF, AF, ZF, SF, OF in architectural bit order.
const FLAG_MASK: u64 = 0x8d5;
const QWORD: MemoryLayout = MemoryLayout {
    ty: Type::I64,
    alignment: 1,
    endian: Endian::Little,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct X86Lifter;

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LiftError {
    Decode(DecodeError),
    Target(TargetError),
    UnsupportedInstruction {
        pc: GuestAddress,
        opcode: String,
        features: CpuFeatureSet,
    },
}

impl fmt::Display for LiftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => error.fmt(f),
            Self::Target(error) => error.fmt(f),
            Self::UnsupportedInstruction {
                pc,
                opcode,
                features,
            } => {
                write!(
                    f,
                    "unsupported x86-64 instruction {opcode} at {:#x} ({features:?})",
                    pc.0
                )
            }
        }
    }
}

impl std::error::Error for LiftError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            Self::Target(error) => Some(error),
            Self::UnsupportedInstruction { .. } => None,
        }
    }
}

impl InstructionLifter for X86Lifter {
    type Error = LiftError;

    fn guest_isa(&self) -> GuestIsa {
        GuestIsa::X86_64
    }

    /// Lifts one instruction. The dispatcher fetches the next instruction after this block exits.
    fn lift_block(
        &self,
        memory: &dyn GuestMemoryRead,
        pc: GuestAddress,
        features: &CpuFeatureSet,
    ) -> Result<IrFunction, Self::Error> {
        self.lift(&fetch(memory, pc).map_err(LiftError::Decode)?, features)
    }
}

impl X86Lifter {
    pub fn lift(
        &self,
        instruction: &DecodedInstruction,
        features: &CpuFeatureSet,
    ) -> Result<IrFunction, LiftError> {
        let target = GuestTarget::new(
            GuestIsa::X86_64,
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
        let exit = match *instruction.operation() {
            Operation::Nop => ir.fallthrough(next),
            Operation::Mov { dst, src } => {
                let value = ir.operand(src, next);
                match dst {
                    Operand::Register(reg) => ir.write(gpr(reg), value),
                    Operand::Memory(address) => {
                        let address = ir.effective_address(address, next);
                        ir.store(address, value);
                    }
                    Operand::Immediate(_) => {
                        unreachable!("decoder only emits register or memory MOV destinations")
                    }
                }
                ir.fallthrough(next)
            }
            Operation::Arithmetic { op, dst, src } => {
                let lhs = ir.read(gpr(dst));
                let rhs = ir.operand(src, next);
                let result = ir.arithmetic(op, lhs, rhs);
                if op != Arithmetic::Compare {
                    ir.write(gpr(dst), result);
                }
                ir.fallthrough(next)
            }
            Operation::Branch { condition, target } => {
                let target = ir.address(target);
                let when = condition.map(|condition| ir.condition(condition));
                ir.check_target(target, when);
                let destination = if let Some(condition) = when {
                    let fallthrough = ir.address(next);
                    ir.value(|out| Op::Select {
                        out,
                        condition,
                        yes: target,
                        no: fallthrough,
                    })
                } else {
                    target
                };
                Exit::NextPc(destination)
            }
            Operation::Call(target) => {
                let target = ir.address(target);
                ir.check_target(target, None);
                let stack = ir.read(gpr(4));
                let updated = ir.with_immediate(IntOp::Sub, stack, 8);
                let address = ir.int_to_address(updated);
                let return_pc = ir.integer(next.0);
                ir.store(address, return_pc);
                ir.write(gpr(4), updated);
                Exit::NextPc(target)
            }
            Operation::Return => {
                let stack = ir.read(gpr(4));
                let address = ir.int_to_address(stack);
                let return_pc = ir.load(address);
                let target = ir.int_to_address(return_pc);
                ir.check_target(target, None);
                let updated = ir.with_immediate(IntOp::Add, stack, 8);
                ir.write(gpr(4), updated);
                Exit::NextPc(target)
            }
            Operation::Ud2 => Exit::Fault(FaultClass::IllegalInstruction),
            Operation::Unsupported { ref opcode } => {
                return Err(LiftError::UnsupportedInstruction {
                    pc,
                    opcode: opcode.clone(),
                    features: features.clone(),
                });
            }
            _ => {
                return Err(LiftError::UnsupportedInstruction {
                    pc,
                    opcode: format!("{:?}", instruction.operation()),
                    features: features.clone(),
                });
            }
        };

        let id = BlockId(0);
        Ok(IrFunction {
            target,
            entry: id,
            blocks: vec![IrBlock {
                id,
                span: CodeSpan {
                    start: pc,
                    len: instruction.bytes().len() as u32,
                },
                parameters: Vec::new(),
                instructions: ir.instructions,
                exit_pc: pc,
                exit,
            }],
        })
    }
}

const fn gpr(index: u8) -> Register {
    Register::X86(X86Reg::Gpr(index))
}
const FLAGS: Register = Register::X86(X86Reg::Rflags);

struct Builder {
    pc: GuestAddress,
    instructions: Vec<Instruction>,
}

impl Builder {
    fn emit(&mut self, op: Op) {
        self.instructions.push(Instruction { pc: self.pc, op });
    }

    fn value(&mut self, operation: impl FnOnce(ValueId) -> Op) -> ValueId {
        let out = ValueId(self.instructions.len() as u32);
        self.emit(operation(out));
        out
    }

    fn integer(&mut self, bits: u64) -> ValueId {
        self.constant(Type::I64, bits)
    }

    fn address(&mut self, address: GuestAddress) -> ValueId {
        self.constant(Type::GuestAddress, address.0)
    }

    fn constant(&mut self, ty: Type, bits: u64) -> ValueId {
        self.value(|out| Op::Const {
            out,
            ty,
            bits: u128::from(bits),
        })
    }

    fn binary(&mut self, op: IntOp, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.value(|out| Op::Int { out, op, lhs, rhs })
    }

    fn with_immediate(&mut self, op: IntOp, lhs: ValueId, rhs: u64) -> ValueId {
        let rhs = self.integer(rhs);
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

    fn right(&mut self, value: ValueId, count: u64) -> ValueId {
        let count = self.integer(count);
        self.value(|out| Op::Shift {
            out,
            op: ShiftOp::LogicalRight,
            value,
            count,
            rule: ShiftCount::Mask6,
        })
    }

    fn read(&mut self, register: Register) -> ValueId {
        self.value(|out| Op::ReadRegister { out, register })
    }

    fn write(&mut self, register: Register, value: ValueId) {
        self.emit(Op::WriteRegister { register, value });
    }

    fn int_to_address(&mut self, value: ValueId) -> ValueId {
        self.value(|out| Op::IntToAddress { out, value })
    }

    fn check_target(&mut self, address: ValueId, when: Option<ValueId>) {
        self.emit(Op::CheckTarget {
            address,
            when,
            constraint: AddressConstraint::Canonical48,
        });
    }

    fn fallthrough(&mut self, next: GuestAddress) -> Exit {
        Exit::NextPc(self.address(next))
    }

    fn effective_address(&mut self, address: MemoryOperand, next: GuestAddress) -> ValueId {
        let value = match address.base {
            MemoryBase::None => self.integer(address.displacement),
            MemoryBase::Rip => self.integer(next.0.wrapping_add(address.displacement)),
            MemoryBase::Register(reg) => {
                let base = self.read(gpr(reg));
                self.with_immediate(IntOp::Add, base, address.displacement)
            }
        };
        let value = match address.index {
            Some((reg, scale)) => {
                let index = self.read(gpr(reg));
                let scaled = self.with_immediate(IntOp::Mul, index, u64::from(scale));
                self.binary(IntOp::Add, value, scaled)
            }
            None => value,
        };
        self.int_to_address(value)
    }

    fn operand(&mut self, operand: Operand, next: GuestAddress) -> ValueId {
        match operand {
            Operand::Register(index) => self.read(gpr(index)),
            Operand::Immediate(bits) => self.integer(bits),
            Operand::Memory(address) => {
                let address = self.effective_address(address, next);
                self.load(address)
            }
        }
    }

    fn load(&mut self, address: ValueId) -> ValueId {
        let guard = self.value(|out| Op::Guard {
            out,
            address,
            access: MemoryAccess::Read,
            layout: QWORD,
        });
        self.value(|out| Op::Load {
            out,
            guard,
            layout: QWORD,
        })
    }

    fn store(&mut self, address: ValueId, value: ValueId) {
        let guard = self.value(|out| Op::Guard {
            out,
            address,
            access: MemoryAccess::Write,
            layout: QWORD,
        });
        self.emit(Op::Store {
            guard,
            value,
            layout: QWORD,
        });
    }

    fn mask_if(&mut self, condition: ValueId, mask: u64, zero: ValueId) -> ValueId {
        let bits = self.integer(mask);
        self.value(|out| Op::Select {
            out,
            condition,
            yes: bits,
            no: zero,
        })
    }

    fn arithmetic(&mut self, op: Arithmetic, lhs: ValueId, rhs: ValueId) -> ValueId {
        let add = op == Arithmetic::Add;
        let result = self.binary(if add { IntOp::Add } else { IntOp::Sub }, lhs, rhs);
        let zero = self.integer(0);

        let carry = if add {
            self.compare(IntPredicate::UnsignedLt, result, lhs)
        } else {
            self.compare(IntPredicate::UnsignedLt, lhs, rhs)
        };
        let cf = self.mask_if(carry, 1, zero);

        let mut parity = result;
        for shift in [4, 2, 1] {
            let shifted = self.right(parity, shift);
            parity = self.binary(IntOp::Xor, parity, shifted);
        }
        let parity = self.with_immediate(IntOp::And, parity, 1);
        let even = self.compare(IntPredicate::Eq, parity, zero);
        let pf = self.mask_if(even, 1 << 2, zero);

        let differing = self.binary(IntOp::Xor, lhs, rhs);
        let auxiliary = self.binary(IntOp::Xor, differing, result);
        let af = self.with_immediate(IntOp::And, auxiliary, 1 << 4);

        let is_zero = self.compare(IntPredicate::Eq, result, zero);
        let zf = self.mask_if(is_zero, 1 << 6, zero);
        let sign = self.right(result, 56);
        let sf = self.with_immediate(IntOp::And, sign, 1 << 7);

        let lhs_result = self.binary(IntOp::Xor, lhs, result);
        let rhs_difference = if add {
            self.binary(IntOp::Xor, rhs, result)
        } else {
            differing
        };
        let overflow = self.binary(IntOp::And, lhs_result, rhs_difference);
        let overflow = self.right(overflow, 52);
        let of = self.with_immediate(IntOp::And, overflow, 1 << 11);

        let old = self.read(FLAGS);
        let mut flags = self.with_immediate(IntOp::And, old, !FLAG_MASK);
        for bit in [cf, pf, af, zf, sf, of] {
            flags = self.binary(IntOp::Or, flags, bit);
        }
        self.write(FLAGS, flags);
        result
    }

    fn condition(&mut self, condition: Condition) -> ValueId {
        let flags = self.read(FLAGS);
        let zero = self.integer(0);
        let (lhs, rhs, predicate) = match condition {
            Condition::Equal | Condition::NotEqual => (
                self.with_immediate(IntOp::And, flags, 1 << 6),
                zero,
                if condition == Condition::Equal {
                    IntPredicate::Ne
                } else {
                    IntPredicate::Eq
                },
            ),
            Condition::Below | Condition::AboveOrEqual => (
                self.with_immediate(IntOp::And, flags, 1),
                zero,
                if condition == Condition::Below {
                    IntPredicate::Ne
                } else {
                    IntPredicate::Eq
                },
            ),
            Condition::Less | Condition::GreaterOrEqual => {
                let sf = self.with_immediate(IntOp::And, flags, 1 << 7);
                let overflow = self.right(flags, 4);
                let of = self.with_immediate(IntOp::And, overflow, 1 << 7);
                (
                    sf,
                    of,
                    if condition == Condition::Less {
                        IntPredicate::Ne
                    } else {
                        IntPredicate::Eq
                    },
                )
            }
        };
        self.compare(predicate, lhs, rhs)
    }
}
