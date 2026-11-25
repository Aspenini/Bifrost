//! Lift the bare ARM64 scalar profile to verified-backend-neutral Bifröst IR.

use bifrost_core::{
    AbiId, Arm64PointerAuth, CpuFeatureSet, FaultClass, GuestAddress, GuestIsa, GuestMemoryRead,
    GuestOs, GuestTarget, TargetError,
};
use bifrost_decoder_arm64::{
    AddressExtend, Bitfield, ConditionalElse, DecodeError, DecodedInstruction, Logical, Operand,
    Operation, PairAddressing, Reg, RegisterShift, fetch,
};
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
    PointerAuth {
        pc: GuestAddress,
        operation: Arm64PointerAuth,
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
            Self::PointerAuth { pc, .. } => {
                write!(f, "ARM64 pointer authentication at {:#x}", pc.0)
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
            next_id: 0,
        };
        let exit = match instruction.operation() {
            Operation::Nop => ir.next(next),
            Operation::Move { dst, src } => {
                let value = ir.operand(src);
                ir.write(dst, value);
                ir.next(next)
            }
            Operation::MoveWord { dst, src } => {
                let source = ir.read(src);
                let value = ir.with_immediate(IntOp::And, source, u64::from(u32::MAX));
                ir.write(dst, value);
                ir.next(next)
            }
            Operation::MoveKeep { dst, bits, shift }
            | Operation::MoveKeepWord { dst, bits, shift } => {
                let current = ir.read(dst);
                let limit = if matches!(instruction.operation(), Operation::MoveKeepWord { .. }) {
                    u64::from(u32::MAX)
                } else {
                    u64::MAX
                };
                let kept = ir.with_immediate(IntOp::And, current, !(0xffff_u64 << shift) & limit);
                let result = ir.with_immediate(IntOp::Or, kept, u64::from(bits) << shift);
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::LogicalImmediate {
                kind,
                dst,
                src,
                mask,
                word,
            } => {
                let lhs = ir.read(src);
                let rhs = ir.integer(mask);
                ir.logical(kind, dst, lhs, rhs, word);
                ir.next(next)
            }
            Operation::LogicalRegister {
                kind,
                dst,
                lhs,
                rhs,
                shift,
                amount,
                invert,
                word,
            } => {
                let lhs = ir.read(lhs);
                let rhs = ir.read(rhs);
                let mut rhs = ir.shift_register(rhs, shift, amount, word);
                if invert {
                    rhs = ir.with_immediate(IntOp::Xor, rhs, u64::MAX);
                }
                ir.logical(kind, dst, lhs, rhs, word);
                ir.next(next)
            }
            Operation::Bitfield {
                kind,
                dst,
                src,
                src_start,
                dst_start,
                len,
                word,
            } => {
                let mask = u64::MAX >> (64 - len);
                let source = ir.read(src);
                let field =
                    ir.shift_register(source, RegisterShift::LogicalRight, src_start, false);
                let field = ir.with_immediate(IntOp::And, field, mask);
                let field = ir.shift_register(field, RegisterShift::Left, dst_start, false);
                let mut result = match kind {
                    Bitfield::Signed => {
                        let padding = 64 - dst_start - len;
                        let value = ir.shift_register(field, RegisterShift::Left, padding, false);
                        ir.shift_register(value, RegisterShift::ArithmeticRight, padding, false)
                    }
                    Bitfield::Merge => {
                        let original = ir.read(dst);
                        let kept = ir.with_immediate(IntOp::And, original, !(mask << dst_start));
                        ir.binary(IntOp::Or, field, kept)
                    }
                    Bitfield::Unsigned => field,
                };
                if word {
                    result = ir.with_immediate(IntOp::And, result, u64::from(u32::MAX));
                }
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::ReverseBits { dst, src, word } => {
                let value = ir.read(src);
                let value = if word {
                    ir.cast(value, Type::I32)
                } else {
                    value
                };
                let result = ir.value(|out| Op::ReverseBits { out, value });
                let result = if word {
                    ir.cast(result, Type::I64)
                } else {
                    result
                };
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::CountLeadingBits {
                dst,
                src,
                word,
                sign,
            } => {
                let value = ir.read(src);
                let value = if word {
                    ir.cast(value, Type::I32)
                } else {
                    value
                };
                let result = ir.value(|out| Op::CountLeadingBits { out, value, sign });
                let result = if word {
                    ir.cast(result, Type::I64)
                } else {
                    result
                };
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::VariableShift {
                dst,
                src,
                count,
                shift,
                word,
            } => {
                let value = ir.read(src);
                let count = ir.read(count);
                let result = ir.shifted(value, shift, count, word);
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::Arithmetic {
                dst,
                lhs,
                rhs,
                sub,
                flags,
            }
            | Operation::ArithmeticWord {
                dst,
                lhs,
                rhs,
                sub,
                flags,
            } => {
                let word = matches!(instruction.operation(), Operation::ArithmeticWord { .. });
                let lhs = ir.read(lhs);
                let rhs = ir.operand(rhs);
                ir.arithmetic(dst, lhs, rhs, sub, flags, word);
                ir.next(next)
            }
            Operation::ArithmeticShifted {
                dst,
                lhs,
                rhs,
                shift,
                amount,
                sub,
                flags,
                word,
            } => {
                let lhs = ir.read(lhs);
                let rhs = ir.read(rhs);
                let rhs = ir.shift_register(rhs, shift, amount, word);
                ir.arithmetic(dst, lhs, rhs, sub, flags, word);
                ir.next(next)
            }
            Operation::MultiplyAccumulate {
                dst,
                lhs,
                rhs,
                accumulator,
                sub,
                word,
                extend,
            } => {
                let lhs = ir.read(lhs);
                let rhs = ir.read(rhs);
                let lhs = ir.extend_word(lhs, extend);
                let rhs = ir.extend_word(rhs, extend);
                let accumulator = ir.read(accumulator);
                let product = ir.binary(IntOp::Mul, lhs, rhs);
                let mut result = ir.binary(
                    if sub { IntOp::Sub } else { IntOp::Add },
                    accumulator,
                    product,
                );
                if word {
                    result = ir.with_immediate(IntOp::And, result, u64::from(u32::MAX));
                }
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::MultiplyHigh {
                dst,
                lhs,
                rhs,
                signed,
            } => {
                let lhs = ir.read(lhs);
                let rhs = ir.read(rhs);
                let op = if signed {
                    IntOp::MulHighSigned
                } else {
                    IntOp::MulHighUnsigned
                };
                let result = ir.binary(op, lhs, rhs);
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::Divide {
                dst,
                lhs,
                rhs,
                signed,
                word,
            } => {
                let lhs = ir.read(lhs);
                let rhs = ir.read(rhs);
                let extend = match (word, signed) {
                    (true, true) => AddressExtend::SignedWord,
                    (true, false) => AddressExtend::UnsignedWord,
                    (false, _) => AddressExtend::Doubleword,
                };
                let lhs = ir.extend_word(lhs, extend);
                let rhs = ir.extend_word(rhs, extend);
                let op = if signed {
                    IntOp::DivSigned
                } else {
                    IntOp::DivUnsigned
                };
                let mut result = ir.binary(op, lhs, rhs);
                if word {
                    result = ir.with_immediate(IntOp::And, result, u64::from(u32::MAX));
                }
                ir.write(dst, result);
                ir.next(next)
            }
            Operation::Memory {
                reg,
                base,
                offset,
                load,
            }
            | Operation::ByteMemory {
                reg,
                base,
                offset,
                load,
            }
            | Operation::WordMemory {
                reg,
                base,
                offset,
                load,
            } => {
                let ty = match instruction.operation() {
                    Operation::ByteMemory { .. } => Type::I8,
                    Operation::WordMemory { .. } => Type::I32,
                    _ => Type::I64,
                };
                let base = ir.memory_base(base, load);
                let address = ir.with_immediate(IntOp::Add, base, offset);
                ir.access_scalar(reg, address, ty, load);
                ir.next(next)
            }
            Operation::IndexedMemory {
                reg,
                base,
                offset,
                load,
                size_log2,
                pre_index,
            } => {
                let original = ir.memory_base(base, load);
                let updated = ir.with_immediate(IntOp::Add, original, offset);
                ir.access_scalar(
                    reg,
                    if pre_index { updated } else { original },
                    [Type::I8, Type::I16, Type::I32, Type::I64][usize::from(size_log2)],
                    load,
                );
                ir.write(base, updated);
                ir.next(next)
            }
            Operation::RegisterOffsetMemory {
                reg,
                base,
                index,
                extend,
                scale,
                size_log2,
                load,
            } => {
                let base = ir.memory_base(base, load);
                let index = ir.read(index);
                let index = ir.extend_word(index, extend);
                let offset = ir.shift_register(
                    index,
                    RegisterShift::Left,
                    if scale { size_log2 } else { 0 },
                    false,
                );
                let address = ir.binary(IntOp::Add, base, offset);
                let ty = [Type::I8, Type::I16, Type::I32, Type::I64][usize::from(size_log2)];
                ir.access_scalar(reg, address, ty, load);
                ir.next(next)
            }
            Operation::WordPair {
                first,
                second,
                base,
                offset,
                load,
                addressing,
            }
            | Operation::DoublePair {
                first,
                second,
                base,
                offset,
                load,
                addressing,
            } => {
                let double = matches!(instruction.operation(), Operation::DoublePair { .. });
                let layout = if double {
                    MemoryLayout::I64_PAIR_LE
                } else {
                    QWORD
                };
                let original = ir.memory_base(base, load);
                let updated = ir.with_immediate(IntOp::Add, original, offset);
                let address = if addressing == PairAddressing::PostIndex {
                    original
                } else {
                    updated
                };
                let address = ir.address(address);
                let access = if load {
                    MemoryAccess::Read
                } else {
                    MemoryAccess::Write
                };
                // Guard the entire pair before changing memory or registers.
                let guard = ir.value(|out| Op::Guard {
                    out,
                    address,
                    access,
                    layout,
                });
                if double && load {
                    let low = ir.fresh_value();
                    let high = ir.fresh_value();
                    ir.emit(Op::LoadPair64 {
                        first: low,
                        second: high,
                        guard,
                    });
                    ir.write(first, low);
                    ir.write(second, high);
                } else if double {
                    let first = ir.read(first);
                    let second = ir.read(second);
                    ir.emit(Op::StorePair64 {
                        first,
                        second,
                        guard,
                    });
                } else if load {
                    let pair = ir.value(|out| Op::Load {
                        out,
                        guard,
                        layout: QWORD,
                    });
                    let low = ir.with_immediate(IntOp::And, pair, u64::from(u32::MAX));
                    let high = ir.shifted_mask(pair, 32, u64::from(u32::MAX));
                    ir.write(first, low);
                    ir.write(second, high);
                } else {
                    let low = ir.read(first);
                    let low = ir.with_immediate(IntOp::And, low, u64::from(u32::MAX));
                    let high = ir.read(second);
                    let count = ir.integer(32);
                    let high = ir.value(|out| Op::Shift {
                        out,
                        op: ShiftOp::Left,
                        value: high,
                        count,
                        rule: ShiftCount::Mask6,
                    });
                    let value = ir.binary(IntOp::Or, low, high);
                    ir.emit(Op::Store {
                        guard,
                        value,
                        layout: QWORD,
                    });
                }
                if addressing != PairAddressing::Offset {
                    ir.write(base, updated);
                }
                ir.next(next)
            }
            Operation::ConditionalCompare {
                lhs,
                rhs,
                condition,
                fallback,
                sub,
                word,
            } => {
                let lhs = ir.read(lhs);
                let rhs = ir.operand(rhs);
                let (lhs, rhs, result) = ir.arithmetic_values(lhs, rhs, sub, word);
                let yes = ir.arithmetic_flags(lhs, rhs, result, sub, if word { 0 } else { 32 });
                let no = ir.integer(u64::from(fallback) << 28);
                let condition = ir.condition(condition);
                let flags = ir.value(|out| Op::Select {
                    out,
                    condition,
                    yes,
                    no,
                });
                ir.write_flags(flags);
                ir.next(next)
            }
            Operation::ConditionalSelect {
                dst,
                lhs,
                rhs,
                condition,
                otherwise,
                word,
            } => {
                let yes = ir.read(lhs);
                let rhs = ir.read(rhs);
                let no = match otherwise {
                    ConditionalElse::Keep => rhs,
                    ConditionalElse::Increment => ir.with_immediate(IntOp::Add, rhs, 1),
                    ConditionalElse::Invert => ir.with_immediate(IntOp::Xor, rhs, u64::MAX),
                    ConditionalElse::Negate => {
                        let zero = ir.integer(0);
                        ir.binary(IntOp::Sub, zero, rhs)
                    }
                };
                let condition = ir.condition(condition);
                let mut result = ir.value(|out| Op::Select {
                    out,
                    condition,
                    yes,
                    no,
                });
                if word {
                    result = ir.with_immediate(IntOp::And, result, u64::from(u32::MAX));
                }
                ir.write(dst, result);
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
            }
            | Operation::BranchZeroWord {
                target,
                reg,
                nonzero,
            } => {
                let mut value = ir.read(reg);
                if matches!(instruction.operation(), Operation::BranchZeroWord { .. }) {
                    value = ir.with_immediate(IntOp::And, value, u64::from(u32::MAX));
                }
                let take = ir.test_zero(value, nonzero);
                ir.branch(take, target, next)
            }
            Operation::BranchBit {
                target,
                reg,
                bit,
                nonzero,
            } => {
                let value = ir.read(reg);
                let value = ir.with_immediate(IntOp::And, value, 1 << bit);
                let take = ir.test_zero(value, nonzero);
                ir.branch(take, target, next)
            }
            Operation::Return(reg) | Operation::BranchRegister { reg, .. } => {
                let destination = ir.read(reg);
                if matches!(
                    instruction.operation(),
                    Operation::BranchRegister { link: true, .. }
                ) {
                    let lr = ir.integer(next.0);
                    ir.write(Reg::X(30), lr);
                }
                Exit::NextPc(ir.address(destination))
            }
            Operation::SupervisorCall { immediate } => {
                return Err(LiftError::SupervisorCall { pc, immediate });
            }
            Operation::PointerAuth(operation) => {
                return Err(LiftError::PointerAuth { pc, operation });
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
    next_id: u32,
}

impl Builder {
    fn emit(&mut self, op: Op) {
        self.instructions.push(Instruction { pc: self.pc, op });
    }

    fn value(&mut self, make: impl FnOnce(ValueId) -> Op) -> ValueId {
        let out = self.fresh_value();
        self.emit(make(out));
        out
    }

    fn fresh_value(&mut self) -> ValueId {
        let id = ValueId(self.next_id);
        self.next_id += 1;
        id
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

    fn test_zero(&mut self, value: ValueId, nonzero: bool) -> ValueId {
        let zero = self.integer(0);
        self.compare(
            if nonzero {
                IntPredicate::Ne
            } else {
                IntPredicate::Eq
            },
            value,
            zero,
        )
    }

    fn cast(&mut self, value: ValueId, ty: Type) -> ValueId {
        self.value(|out| Op::IntCast { out, value, ty })
    }

    fn shift_register(
        &mut self,
        value: ValueId,
        shift: RegisterShift,
        amount: u8,
        word: bool,
    ) -> ValueId {
        if amount == 0 {
            return value;
        }
        let count = self.integer(u64::from(amount));
        self.shifted(value, shift, count, word)
    }

    fn shifted(
        &mut self,
        value: ValueId,
        shift: RegisterShift,
        count: ValueId,
        word: bool,
    ) -> ValueId {
        let value = if word {
            self.cast(value, Type::I32)
        } else {
            value
        };
        let op = match shift {
            RegisterShift::Left => ShiftOp::Left,
            RegisterShift::LogicalRight => ShiftOp::LogicalRight,
            RegisterShift::ArithmeticRight => ShiftOp::ArithmeticRight,
            RegisterShift::RotateRight => ShiftOp::RotateRight,
        };
        let result = self.value(|out| Op::Shift {
            out,
            value,
            count,
            op,
            rule: ShiftCount::ModuloWidth,
        });
        if word {
            self.cast(result, Type::I64)
        } else {
            result
        }
    }

    fn logical(&mut self, kind: Logical, dst: Reg, lhs: ValueId, rhs: ValueId, word: bool) {
        let op = match kind {
            Logical::And | Logical::Ands => IntOp::And,
            Logical::Orr => IntOp::Or,
            Logical::Eor => IntOp::Xor,
        };
        let mut result = self.binary(op, lhs, rhs);
        if word {
            result = self.with_immediate(IntOp::And, result, u64::from(u32::MAX));
        }
        if kind == Logical::Ands {
            let flags = self.nz_flags(result, if word { 0 } else { 32 });
            self.write_flags(flags);
        }
        self.write(dst, result);
    }

    fn arithmetic(
        &mut self,
        dst: Reg,
        lhs: ValueId,
        rhs: ValueId,
        sub: bool,
        flags: bool,
        word: bool,
    ) {
        let (lhs, rhs, result) = self.arithmetic_values(lhs, rhs, sub, word);
        if flags {
            let flags = self.arithmetic_flags(lhs, rhs, result, sub, if word { 0 } else { 32 });
            self.write_flags(flags);
        }
        self.write(dst, result);
    }

    fn arithmetic_values(
        &mut self,
        mut lhs: ValueId,
        mut rhs: ValueId,
        sub: bool,
        word: bool,
    ) -> (ValueId, ValueId, ValueId) {
        if word {
            lhs = self.with_immediate(IntOp::And, lhs, u64::from(u32::MAX));
            rhs = self.with_immediate(IntOp::And, rhs, u64::from(u32::MAX));
        }
        let mut result = self.binary(if sub { IntOp::Sub } else { IntOp::Add }, lhs, rhs);
        if word {
            result = self.with_immediate(IntOp::And, result, u64::from(u32::MAX));
        }
        (lhs, rhs, result)
    }

    fn access_scalar(&mut self, reg: Reg, address: ValueId, ty: Type, load: bool) {
        let layout = MemoryLayout { ty, ..QWORD };
        let address = self.address(address);
        let access = if load {
            MemoryAccess::Read
        } else {
            MemoryAccess::Write
        };
        let guard = self.value(|out| Op::Guard {
            out,
            address,
            access,
            layout,
        });
        if load {
            let value = self.value(|out| Op::Load { out, guard, layout });
            let value = if ty != Type::I64 {
                self.cast(value, Type::I64)
            } else {
                value
            };
            self.write(reg, value);
        } else {
            let value = self.read(reg);
            let value = if ty != Type::I64 {
                self.cast(value, ty)
            } else {
                value
            };
            self.emit(Op::Store {
                guard,
                value,
                layout,
            });
        }
    }

    fn extend_word(&mut self, value: ValueId, extend: AddressExtend) -> ValueId {
        match extend {
            AddressExtend::UnsignedWord => {
                self.with_immediate(IntOp::And, value, u64::from(u32::MAX))
            }
            AddressExtend::SignedWord => {
                let low = self.shift_register(value, RegisterShift::Left, 32, false);
                self.shift_register(low, RegisterShift::ArithmeticRight, 32, false)
            }
            AddressExtend::Doubleword => value,
        }
    }

    fn memory_base(&mut self, base: Reg, load: bool) -> ValueId {
        let value = self.read(base);
        if base == Reg::Sp {
            let address = self.address(value);
            let access = if load {
                MemoryAccess::Read
            } else {
                MemoryAccess::Write
            };
            self.emit(Op::CheckMemoryAlignment {
                address,
                alignment: 16,
                access,
            });
        }
        value
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

    fn nz_flags(&mut self, result: ValueId, sign_shift: u64) -> ValueId {
        let zero = self.integer(0);
        let n = self.shifted_mask(result, sign_shift, 1 << 31);
        let is_zero = self.compare(IntPredicate::Eq, result, zero);
        let z = self.mask_if(is_zero, 1 << 30, zero);
        self.binary(IntOp::Or, n, z)
    }

    fn arithmetic_flags(
        &mut self,
        lhs: ValueId,
        rhs: ValueId,
        result: ValueId,
        sub: bool,
        sign_shift: u64,
    ) -> ValueId {
        let nz = self.nz_flags(result, sign_shift);
        let zero = self.integer(0);
        let carry = if sub {
            self.compare(IntPredicate::UnsignedLe, rhs, lhs)
        } else {
            self.compare(IntPredicate::UnsignedLt, result, lhs)
        };
        let c = self.mask_if(carry, 1 << 29, zero);
        let sign_change = self.binary(IntOp::Xor, lhs, result);
        let affected = self.binary(IntOp::Xor, if sub { lhs } else { result }, rhs);
        let overflow = self.binary(IntOp::And, sign_change, affected);
        let v = self.shifted_mask(overflow, sign_shift + 3, 1 << 28);
        let cv = self.binary(IntOp::Or, c, v);
        self.binary(IntOp::Or, nz, cv)
    }

    fn write_flags(&mut self, flags: ValueId) {
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
