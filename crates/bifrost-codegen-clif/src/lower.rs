use crate::CompileError;
use bifrost_core::{FaultClass, GuestAddress};
use bifrost_ir::{
    AddressConstraint, Arm64Reg, Endian, Exit, IntOp, IntPredicate, IrBlock, MemoryLayout, Op,
    Register, ShiftCount, ShiftOp, Type, ValueId, X86Reg,
};
use cranelift_codegen::{
    ir::{
        AbiParam, Function, InstBuilder, MemFlagsData, SigRef, Signature, Value, condcodes::IntCC,
        types,
    },
    isa::TargetIsa,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use std::collections::BTreeMap;

// The frame consists only of consecutive u64 slots. The host call site owns it.
pub(crate) const STACK: i32 = 31 * 8;
pub(crate) const FLAGS: i32 = 32 * 8;
pub(crate) const NEXT_PC: i32 = 33 * 8;
pub(crate) const FAULT_DETAIL: i32 = 34 * 8;
pub(crate) const HELPER_VALUE: i32 = 35 * 8;
pub(crate) const HELPER_HIGH: i32 = 36 * 8;
pub(crate) const DISPATCH: i64 = 0;
pub(crate) const NONCANONICAL: i64 = 1;
pub(crate) const ILLEGAL: i64 = 2;
pub(crate) const TRAP: i64 = 3;
pub(crate) const HELPER_FAILED: i64 = 4;

pub(crate) fn into_function(
    function: &mut Function,
    isa: &dyn TargetIsa,
    block: &IrBlock,
) -> Result<(), CompileError> {
    function.signature.call_conv = isa.default_call_conv();
    for _ in 0..3 {
        function
            .signature
            .params
            .push(AbiParam::new(isa.pointer_type()));
    }
    function.signature.returns.push(AbiParam::new(types::I32));
    let mut context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(function, &mut context);
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    let frame = builder.block_params(entry)[0];
    let memory = builder.block_params(entry)[1];
    let helper = builder.block_params(entry)[2];
    let mut signature = Signature::new(isa.default_call_conv());
    for ty in [
        isa.pointer_type(),
        isa.pointer_type(),
        types::I64,
        types::I64,
        types::I32,
    ] {
        signature.params.push(AbiParam::new(ty));
    }
    signature.returns.push(AbiParam::new(types::I32));
    let helper_signature = builder.import_signature(signature);
    let mut lowering = Lowering {
        builder,
        frame,
        memory,
        helper,
        helper_signature,
        values: BTreeMap::new(),
        guards: BTreeMap::new(),
        pc: block.span.start,
    };
    for instruction in &block.instructions {
        lowering.instruction(instruction.op)?;
    }
    lowering.exit(&block.exit)?;
    lowering.builder.seal_all_blocks();
    lowering.builder.finalize(isa.frontend_config());
    Ok(())
}

/// Scalar values are held in I64 with unused upper bits cleared.
#[derive(Clone, Copy)]
struct Scalar {
    value: Value,
    width: u32,
}

struct Lowering<'a> {
    builder: FunctionBuilder<'a>,
    frame: Value,
    memory: Value,
    helper: Value,
    helper_signature: SigRef,
    values: BTreeMap<ValueId, Scalar>,
    guards: BTreeMap<ValueId, Value>,
    pc: GuestAddress,
}

impl Lowering<'_> {
    fn unsupported(&self, operation: &'static str) -> CompileError {
        CompileError::UnsupportedOperation {
            pc: self.pc,
            operation,
        }
    }

    fn instruction(&mut self, op: Op) -> Result<(), CompileError> {
        let (out, value, width) = match op {
            Op::RequireFeature(_) => return Ok(()),
            Op::Fence => {
                self.builder.ins().fence();
                return Ok(());
            }
            Op::Const { out, ty, bits } => {
                match ty {
                    Type::I1
                    | Type::I8
                    | Type::I16
                    | Type::I32
                    | Type::I64
                    | Type::GuestAddress => (),
                    _ => return Err(self.unsupported("constant type")),
                }
                (out, self.immediate(bits as i64), ty.bit_width())
            }
            Op::Int { out, op, lhs, rhs } => {
                let lhs = self.values[&lhs];
                let rhs = self.values[&rhs].value;
                let ins = self.builder.ins();
                let value = match op {
                    IntOp::Add => ins.iadd(lhs.value, rhs),
                    IntOp::Sub => ins.isub(lhs.value, rhs),
                    IntOp::Mul => ins.imul(lhs.value, rhs),
                    IntOp::And => ins.band(lhs.value, rhs),
                    IntOp::Or => ins.bor(lhs.value, rhs),
                    IntOp::Xor => ins.bxor(lhs.value, rhs),
                };
                (out, value, lhs.width)
            }
            Op::Compare {
                out,
                predicate,
                lhs,
                rhs,
            } => {
                let mut lhs = self.values[&lhs];
                let mut rhs = self.values[&rhs];
                let condition = match predicate {
                    IntPredicate::Eq => IntCC::Equal,
                    IntPredicate::Ne => IntCC::NotEqual,
                    IntPredicate::SignedLt => IntCC::SignedLessThan,
                    IntPredicate::SignedLe => IntCC::SignedLessThanOrEqual,
                    IntPredicate::UnsignedLt => IntCC::UnsignedLessThan,
                    IntPredicate::UnsignedLe => IntCC::UnsignedLessThanOrEqual,
                };
                if matches!(predicate, IntPredicate::SignedLt | IntPredicate::SignedLe) {
                    lhs.value = self.signed(lhs);
                    rhs.value = self.signed(rhs);
                }
                let flag = self.builder.ins().icmp(condition, lhs.value, rhs.value);
                (out, self.builder.ins().uextend(types::I64, flag), 1)
            }
            Op::Shift {
                out,
                op,
                value,
                count,
                rule,
            } => {
                let value = self.values[&value];
                let count = self.values[&count].value;
                (out, self.shift(value, count, op, rule), value.width)
            }
            Op::ReverseBits { out, value } => {
                let Scalar { value, width } = self.values[&value];
                let reversed = self.builder.ins().bitrev(value);
                let reversed = self
                    .builder
                    .ins()
                    .ushr_imm_u(reversed, i64::from(64 - width));
                (out, reversed, width)
            }
            Op::CountLeadingBits { out, value, sign } => {
                let scalar = self.values[&value];
                let bits = if sign {
                    let extended = self.signed(scalar);
                    let fill = self.builder.ins().sshr_imm_u(extended, 63);
                    let bits = self.builder.ins().bxor(scalar.value, fill);
                    self.mask(bits, scalar.width)
                } else {
                    scalar.value
                };
                let count = self.builder.ins().clz(bits);
                let count = self
                    .builder
                    .ins()
                    .iadd_imm_s(count, i64::from(scalar.width) - 64 - i64::from(sign));
                (out, count, scalar.width)
            }
            Op::Select {
                out,
                condition,
                yes,
                no,
            } => {
                let flag = self.values[&condition].value;
                let yes = self.values[&yes];
                let no = self.values[&no].value;
                (
                    out,
                    self.builder.ins().select(flag, yes.value, no),
                    yes.width,
                )
            }
            Op::IntToAddress { out, value } | Op::AddressToInt { out, value } => {
                (out, self.values[&value].value, 64)
            }
            Op::IntCast { out, value, ty } => {
                let width = ty.bit_width();
                if width > 64 {
                    return Err(self.unsupported("integer width"));
                }
                (out, self.values[&value].value, width)
            }
            Op::CheckTarget {
                address,
                when,
                constraint: AddressConstraint::Canonical48,
            } => {
                let address = self.values[&address].value;
                let extended = self.signed(Scalar {
                    value: address,
                    width: 48,
                });
                let mut invalid = self.builder.ins().icmp(IntCC::NotEqual, address, extended);
                if let Some(flag) = when {
                    let taken = self
                        .builder
                        .ins()
                        .ireduce(types::I8, self.values[&flag].value);
                    invalid = self.builder.ins().band(invalid, taken);
                }
                let fault = self.builder.create_block();
                let success = self.builder.create_block();
                self.builder.ins().brif(invalid, fault, &[], success, &[]);
                self.builder.switch_to_block(fault);
                self.store(FAULT_DETAIL, address);
                self.status(NONCANONICAL);
                self.builder.switch_to_block(success);
                return Ok(());
            }
            Op::ReadRegister { out, register } => {
                let (offset, width) = self.register_layout(register)?;
                let value = self.builder.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    self.frame,
                    offset,
                );
                (out, value, width)
            }
            Op::WriteRegister { register, value } => {
                let (offset, _) = self.register_layout(register)?;
                self.store(offset, self.values[&value].value);
                return Ok(());
            }
            Op::Guard { out, address, .. } => {
                self.guards.insert(out, self.values[&address].value);
                return Ok(());
            }
            Op::Load { out, guard, layout } => {
                if layout.ty.bit_width() > 64 {
                    return Err(self.unsupported("memory width"));
                }
                let zero = self.immediate(0);
                self.memory_access(guard, zero, layout, false)?;
                let value = self.builder.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    self.frame,
                    HELPER_VALUE,
                );
                (out, value, layout.ty.bit_width())
            }
            Op::Store {
                guard,
                value,
                layout,
            } => {
                if layout.ty.bit_width() > 64 {
                    return Err(self.unsupported("memory width"));
                }
                return self.memory_access(guard, self.values[&value].value, layout, true);
            }
            Op::LoadPair64 {
                first,
                second,
                guard,
            } => {
                let zero = self.immediate(0);
                self.memory_access(guard, zero, MemoryLayout::I64_PAIR_LE, false)?;
                for (out, offset) in [(first, HELPER_VALUE), (second, HELPER_HIGH)] {
                    let value = self.builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        self.frame,
                        offset,
                    );
                    self.values.insert(out, Scalar { value, width: 64 });
                }
                return Ok(());
            }
            Op::StorePair64 {
                first,
                second,
                guard,
            } => {
                self.store(HELPER_HIGH, self.values[&second].value);
                return self.memory_access(
                    guard,
                    self.values[&first].value,
                    MemoryLayout::I64_PAIR_LE,
                    true,
                );
            }
            _ => return Err(self.unsupported("IR operation")),
        };
        let value = self.mask(value, width);
        self.values.insert(out, Scalar { value, width });
        Ok(())
    }

    fn immediate(&mut self, bits: i64) -> Value {
        self.builder.ins().iconst(types::I64, bits)
    }

    fn memory_access(
        &mut self,
        guard: ValueId,
        value: Value,
        layout: MemoryLayout,
        write: bool,
    ) -> Result<(), CompileError> {
        let width = layout
            .ty
            .storage_bytes()
            .filter(|&size| size <= 16)
            .ok_or_else(|| self.unsupported("memory width"))?;
        let mut options = u32::from(width) | (u32::from(layout.alignment) << 8);
        options |= u32::from(layout.endian == Endian::Big) << 16;
        options |= u32::from(write) << 17;
        let options = self.builder.ins().iconst(types::I32, i64::from(options));
        let call = self.builder.ins().call_indirect(
            self.helper_signature,
            self.helper,
            &[self.frame, self.memory, self.guards[&guard], value, options],
        );
        let status = self.builder.inst_results(call)[0];
        let fault = self.builder.create_block();
        let success = self.builder.create_block();
        self.builder.ins().brif(status, fault, &[], success, &[]);
        self.builder.switch_to_block(fault);
        self.status(HELPER_FAILED);
        self.builder.switch_to_block(success);
        Ok(())
    }

    fn signed(&mut self, scalar: Scalar) -> Value {
        if scalar.width == 64 {
            return scalar.value;
        }
        let padding = i64::from(64 - scalar.width);
        let shifted = self.builder.ins().ishl_imm_u(scalar.value, padding);
        self.builder.ins().sshr_imm_u(shifted, padding)
    }

    fn mask(&mut self, value: Value, width: u32) -> Value {
        if width == 64 {
            value
        } else {
            self.builder.ins().band_imm_u(value, (1_i64 << width) - 1)
        }
    }

    fn shift(&mut self, scalar: Scalar, count: Value, op: ShiftOp, rule: ShiftCount) -> Value {
        let width = i64::from(scalar.width);
        let count = self.builder.ins().band_imm_u(
            count,
            match rule {
                ShiftCount::Mask5 => 31,
                ShiftCount::Mask6 => 63,
                ShiftCount::ModuloWidth => width - 1,
            },
        );
        let value = scalar.value;
        match op {
            ShiftOp::Left | ShiftOp::LogicalRight => {
                let shifted = if op == ShiftOp::Left {
                    self.builder.ins().ishl(value, count)
                } else {
                    self.builder.ins().ushr(value, count)
                };
                let oversized =
                    self.builder
                        .ins()
                        .icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, count, width);
                let zero = self.immediate(0);
                self.builder.ins().select(oversized, zero, shifted)
            }
            ShiftOp::ArithmeticRight => {
                let limit = self.immediate(width - 1);
                let count = self.builder.ins().umin(count, limit);
                let extended = self.signed(scalar);
                self.builder.ins().sshr(extended, count)
            }
            ShiftOp::RotateLeft | ShiftOp::RotateRight => {
                let count = self.builder.ins().band_imm_u(count, width - 1);
                let width = self.immediate(width);
                let inverse = self.builder.ins().isub(width, count);
                let (left_count, right_count) = if op == ShiftOp::RotateLeft {
                    (count, inverse)
                } else {
                    (inverse, count)
                };
                let left = self.builder.ins().ishl(value, left_count);
                let right = self.builder.ins().ushr(value, right_count);
                self.builder.ins().bor(left, right)
            }
        }
    }

    fn register_layout(&self, register: Register) -> Result<(i32, u32), CompileError> {
        match register {
            Register::X86(X86Reg::Gpr(index)) | Register::Arm64(Arm64Reg::X(index)) => {
                Ok((i32::from(index) * 8, 64))
            }
            Register::X86(X86Reg::Rflags) => Ok((FLAGS, 64)),
            Register::Arm64(Arm64Reg::Nzcv) => Ok((FLAGS, 32)),
            Register::Arm64(Arm64Reg::Sp) => Ok((STACK, 64)),
            _ => Err(self.unsupported("register")),
        }
    }

    fn store(&mut self, offset: i32, value: Value) {
        self.builder
            .ins()
            .store(MemFlagsData::trusted(), value, self.frame, offset);
    }

    fn status(&mut self, code: i64) {
        let status = self.builder.ins().iconst(types::I32, code);
        self.builder.ins().return_(&[status]);
    }

    fn exit(&mut self, exit: &Exit) -> Result<(), CompileError> {
        match exit {
            Exit::NextPc(next) => {
                self.store(NEXT_PC, self.values[next].value);
                self.status(DISPATCH);
            }
            Exit::Fault(FaultClass::IllegalInstruction) => self.status(ILLEGAL),
            Exit::Trap { code } | Exit::Fault(FaultClass::Trap(code)) => {
                let detail = self.immediate(i64::from(*code));
                self.store(FAULT_DETAIL, detail);
                self.status(TRAP);
            }
            _ => return Err(self.unsupported("block exit")),
        }
        Ok(())
    }
}
