use crate::{
    Arm64Reg, BlockId, Edge, Exit, IntOp, IrBlock, IrFunction, MemoryAccess, MemoryLayout, Op,
    Register, Type, ValueId,
};
use bifrost_core::{CpuFeature, GuestAddress, GuestTarget};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Immutable proof of structural and type validity. Runtime memory checks are still required.
#[derive(Clone, Debug)]
pub struct VerifiedIrFunction(IrFunction);

impl AsRef<IrFunction> for VerifiedIrFunction {
    fn as_ref(&self) -> &IrFunction {
        &self.0
    }
}

impl VerifiedIrFunction {
    pub fn blocks(&self) -> impl ExactSizeIterator<Item = VerifiedIrBlock<'_>> {
        self.0.blocks.iter().map(|block| VerifiedIrBlock {
            block,
            target: &self.0.target,
        })
    }
}

/// An immutable verified block and its guest target. A backend may reject unsupported operations.
#[derive(Clone, Copy, Debug)]
pub struct VerifiedIrBlock<'a> {
    block: &'a IrBlock,
    target: &'a GuestTarget,
}

impl AsRef<IrBlock> for VerifiedIrBlock<'_> {
    fn as_ref(&self) -> &IrBlock {
        self.block
    }
}

impl VerifiedIrBlock<'_> {
    pub const fn target(&self) -> &GuestTarget {
        self.target
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifyError {
    pub block: BlockId,
    pub pc: Option<GuestAddress>,
    pub kind: VerifyErrorKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum VerifyErrorKind {
    MissingBlock,
    DuplicateBlock,
    UnreachableBlock,
    InvalidSpan,
    PcOutOfOrderOrSpan,
    DuplicateValue(ValueId),
    UndefinedValue(ValueId),
    ExpectedData(ValueId),
    TypeMismatch {
        value: ValueId,
        expected: Type,
        found: Type,
    },
    ExpectedInteger(ValueId),
    ConstantOutOfRange(Type),
    InvalidRegister(Register),
    UndeclaredFeature(CpuFeature),
    InvalidMemoryLayout(MemoryLayout),
    InvalidAlignment(u8),
    IncompatibleGuard(ValueId),
    UnusedGuard(ValueId),
    BranchArity {
        target: BlockId,
        expected: usize,
        found: usize,
    },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid IR in block {}", self.block.0)?;
        if let Some(pc) = self.pc {
            write!(f, " at guest PC {:#x}", pc.0)?;
        }
        write!(f, ": {:?}", self.kind)
    }
}

impl std::error::Error for VerifyError {}

pub(crate) fn verify(function: IrFunction) -> Result<VerifiedIrFunction, VerifyError> {
    let mut blocks = BTreeMap::new();
    for block in &function.blocks {
        if blocks.insert(block.id, block).is_some() {
            return Err(error(block.id, VerifyErrorKind::DuplicateBlock));
        }
    }
    if !blocks.contains_key(&function.entry) {
        return Err(error(function.entry, VerifyErrorKind::MissingBlock));
    }
    for block in &function.blocks {
        verify_block(block, &function.target, &blocks)?;
    }

    let mut visited = BTreeSet::new();
    let mut pending = vec![function.entry];
    while let Some(id) = pending.pop() {
        if visited.insert(id) {
            match &blocks[&id].exit {
                Exit::Jump(edge) => pending.push(edge.target),
                Exit::Branch { yes, no, .. } => pending.extend([yes.target, no.target]),
                Exit::NextPc(_) | Exit::Trap { .. } | Exit::Fault(_) => {}
            }
        }
    }
    if let Some(&id) = blocks.keys().find(|id| !visited.contains(id)) {
        return Err(error(id, VerifyErrorKind::UnreachableBlock));
    }
    Ok(VerifiedIrFunction(function))
}

fn error(block: BlockId, kind: VerifyErrorKind) -> VerifyError {
    VerifyError {
        block,
        pc: None,
        kind,
    }
}

fn verify_block(
    block: &IrBlock,
    target: &GuestTarget,
    blocks: &BTreeMap<BlockId, &IrBlock>,
) -> Result<(), VerifyError> {
    let start = block.span.start;
    let end = block
        .span
        .len
        .checked_sub(1)
        .and_then(|offset| start.checked_add(u64::from(offset)))
        .ok_or_else(|| error(block.id, VerifyErrorKind::InvalidSpan))?;
    let mut values = Values::default();
    for parameter in &block.parameters {
        values
            .define(parameter.value, Value::Data(parameter.ty))
            .map_err(|kind| error(block.id, kind))?;
    }
    let mut previous = start;
    let check_pc = |pc, previous| {
        if pc < previous || pc > end {
            Err(VerifyErrorKind::PcOutOfOrderOrSpan)
        } else {
            Ok(())
        }
    };
    for inst in &block.instructions {
        check_pc(inst.pc, previous)
            .and_then(|()| values.instruction(inst.op, target, inst.pc))
            .map_err(|kind| VerifyError {
                block: block.id,
                pc: Some(inst.pc),
                kind,
            })?;
        previous = inst.pc;
    }
    check_pc(block.exit_pc, previous)
        .and_then(|()| values.no_unused_guards())
        .and_then(|()| values.exit(&block.exit, blocks))
        .map_err(|kind| VerifyError {
            block: block.id,
            pc: Some(block.exit_pc),
            kind,
        })
}

#[derive(Clone, Copy)]
enum Value {
    Data(Type),
    Guard {
        access: MemoryAccess,
        layout: MemoryLayout,
        pc: GuestAddress,
        used: bool,
    },
}

#[derive(Default)]
struct Values(BTreeMap<ValueId, Value>);

impl Values {
    fn define(&mut self, id: ValueId, value: Value) -> Result<(), VerifyErrorKind> {
        if self.0.insert(id, value).is_some() {
            return Err(VerifyErrorKind::DuplicateValue(id));
        }
        Ok(())
    }

    fn data(&self, id: ValueId) -> Result<Type, VerifyErrorKind> {
        match self.0.get(&id) {
            Some(Value::Data(ty)) => Ok(*ty),
            Some(Value::Guard { .. }) => Err(VerifyErrorKind::ExpectedData(id)),
            None => Err(VerifyErrorKind::UndefinedValue(id)),
        }
    }

    fn expect(&self, id: ValueId, expected: Type) -> Result<(), VerifyErrorKind> {
        let found = self.data(id)?;
        if found != expected {
            return Err(VerifyErrorKind::TypeMismatch {
                value: id,
                expected,
                found,
            });
        }
        Ok(())
    }

    fn integer(&self, id: ValueId) -> Result<Type, VerifyErrorKind> {
        let ty = self.data(id)?;
        if !ty.is_integer() {
            return Err(VerifyErrorKind::ExpectedInteger(id));
        }
        Ok(ty)
    }

    fn consume_guard(
        &mut self,
        id: ValueId,
        access: MemoryAccess,
        layout: MemoryLayout,
        pc: GuestAddress,
    ) -> Result<(), VerifyErrorKind> {
        match self.0.get_mut(&id) {
            Some(Value::Guard {
                access: required,
                layout: guarded,
                pc: guard_pc,
                used,
            }) if *required == access && *guarded == layout && *guard_pc == pc && !*used => {
                *used = true;
                Ok(())
            }
            Some(_) => Err(VerifyErrorKind::IncompatibleGuard(id)),
            None => Err(VerifyErrorKind::UndefinedValue(id)),
        }
    }

    fn no_unused_guards(&self) -> Result<(), VerifyErrorKind> {
        for (&id, value) in &self.0 {
            if matches!(value, Value::Guard { used: false, .. }) {
                return Err(VerifyErrorKind::UnusedGuard(id));
            }
        }
        Ok(())
    }

    fn instruction(
        &mut self,
        op: Op,
        target: &GuestTarget,
        pc: GuestAddress,
    ) -> Result<(), VerifyErrorKind> {
        let register_type = |register: Register| {
            register
                .ty_for(target.isa())
                .ok_or(VerifyErrorKind::InvalidRegister(register))
        };
        let (out, ty) = match op {
            Op::Fence => return Ok(()),
            Op::RequireFeature(feature) => {
                return if target.features().contains(feature) {
                    Ok(())
                } else {
                    Err(VerifyErrorKind::UndeclaredFeature(feature))
                };
            }
            Op::Const { out, ty, bits } => {
                if ty.bit_width() < 128 && bits >> ty.bit_width() != 0 {
                    return Err(VerifyErrorKind::ConstantOutOfRange(ty));
                }
                (out, ty)
            }
            Op::Int { out, op, lhs, rhs } => {
                let ty = self.integer(lhs)?;
                if matches!(
                    op,
                    IntOp::MulHighUnsigned
                        | IntOp::MulHighSigned
                        | IntOp::DivUnsigned
                        | IntOp::DivSigned
                ) {
                    self.expect(lhs, Type::I64)?;
                }
                self.expect(rhs, ty)?;
                (out, ty)
            }
            Op::Compare { out, lhs, rhs, .. } => {
                self.expect(rhs, self.integer(lhs)?)?;
                (out, Type::I1)
            }
            Op::Shift {
                out, value, count, ..
            } => {
                let ty = self.integer(value)?;
                self.integer(count)?;
                (out, ty)
            }
            Op::ReverseBits { out, value } | Op::CountLeadingBits { out, value, .. } => {
                (out, self.integer(value)?)
            }
            Op::Select {
                out,
                condition,
                yes,
                no,
            } => {
                self.expect(condition, Type::I1)?;
                let ty = self.data(yes)?;
                self.expect(no, ty)?;
                (out, ty)
            }
            Op::IntToAddress { out, value } => {
                self.expect(value, Type::I64)?;
                (out, Type::GuestAddress)
            }
            Op::AddressToInt { out, value } => {
                self.expect(value, Type::GuestAddress)?;
                (out, Type::I64)
            }
            Op::IntCast { out, value, ty } => {
                self.integer(value)?;
                if !ty.is_integer() {
                    return Err(VerifyErrorKind::ExpectedInteger(out));
                }
                (out, ty)
            }
            Op::CheckTarget { address, when, .. } => {
                self.expect(address, Type::GuestAddress)?;
                return match when {
                    Some(condition) => self.expect(condition, Type::I1),
                    None => Ok(()),
                };
            }
            Op::CheckMemoryAlignment {
                address, alignment, ..
            } => {
                self.expect(address, Type::GuestAddress)?;
                if !alignment.is_power_of_two() {
                    return Err(VerifyErrorKind::InvalidAlignment(alignment));
                }
                return Ok(());
            }
            Op::ReadRegister { out, register } => (out, register_type(register)?),
            Op::WriteRegister { register, value } => {
                if register == Register::Arm64(Arm64Reg::TpidrroEl0) {
                    return Err(VerifyErrorKind::InvalidRegister(register));
                }
                return self.expect(value, register_type(register)?);
            }
            Op::Guard {
                out,
                address,
                access,
                layout,
            } => {
                self.expect(address, Type::GuestAddress)?;
                if !layout.ty.storage_bytes().is_some_and(|width| {
                    layout.alignment.is_power_of_two() && layout.alignment <= width
                }) {
                    return Err(VerifyErrorKind::InvalidMemoryLayout(layout));
                }
                return self.define(
                    out,
                    Value::Guard {
                        access,
                        layout,
                        pc,
                        used: false,
                    },
                );
            }
            Op::Load { out, guard, layout } => {
                self.consume_guard(guard, MemoryAccess::Read, layout, pc)?;
                (out, layout.ty)
            }
            Op::Store {
                guard,
                value,
                layout,
            } => {
                self.expect(value, layout.ty)?;
                return self.consume_guard(guard, MemoryAccess::Write, layout, pc);
            }
            Op::LoadPair64 {
                first,
                second,
                guard,
            } => {
                self.consume_guard(guard, MemoryAccess::Read, MemoryLayout::I64_PAIR_LE, pc)?;
                self.define(first, Value::Data(Type::I64))?;
                (second, Type::I64)
            }
            Op::StorePair64 {
                first,
                second,
                guard,
            } => {
                self.expect(first, Type::I64)?;
                self.expect(second, Type::I64)?;
                return self.consume_guard(
                    guard,
                    MemoryAccess::Write,
                    MemoryLayout::I64_PAIR_LE,
                    pc,
                );
            }
        };
        self.define(out, Value::Data(ty))
    }

    fn exit(
        &self,
        exit: &Exit,
        blocks: &BTreeMap<BlockId, &IrBlock>,
    ) -> Result<(), VerifyErrorKind> {
        match exit {
            Exit::Jump(edge) => self.edge(edge, blocks),
            Exit::Branch { condition, yes, no } => {
                self.expect(*condition, Type::I1)?;
                self.edge(yes, blocks)?;
                self.edge(no, blocks)
            }
            Exit::NextPc(pc) => self.expect(*pc, Type::GuestAddress),
            Exit::Trap { .. } | Exit::Fault(_) => Ok(()),
        }
    }

    fn edge(
        &self,
        edge: &Edge,
        blocks: &BTreeMap<BlockId, &IrBlock>,
    ) -> Result<(), VerifyErrorKind> {
        let target = blocks
            .get(&edge.target)
            .ok_or(VerifyErrorKind::MissingBlock)?;
        if target.parameters.len() != edge.args.len() {
            return Err(VerifyErrorKind::BranchArity {
                target: edge.target,
                expected: target.parameters.len(),
                found: edge.args.len(),
            });
        }
        for (&arg, param) in edge.args.iter().zip(&target.parameters) {
            self.expect(arg, param.ty)?;
        }
        Ok(())
    }
}
