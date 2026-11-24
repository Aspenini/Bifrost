use super::{InterpreterError, Local, Locals, data, guarded_address, value::Value};
use bifrost_core::{
    AccessKind, FaultClass, GuestAddress, GuestFault, GuestMemory, GuestThread, MemoryError,
};
use bifrost_ir::{AddressConstraint, Endian, MemoryLayout, Op, Type};

pub(crate) enum EvalError {
    Guest(GuestFault),
    Backend(InterpreterError),
}

pub(crate) fn instruction(
    op: Op,
    pc: GuestAddress,
    thread: &mut GuestThread,
    memory: &mut dyn GuestMemory,
    locals: &mut Locals,
) -> Result<(), EvalError> {
    let (out, value) = match op {
        Op::RequireFeature(_) => return Ok(()),
        Op::Fence => {
            std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
            return Ok(());
        }
        Op::Const { out, ty, bits } => (out, Value::new(ty, bits)),
        Op::Int { out, op, lhs, rhs } => (out, data(locals, lhs).integer(op, data(locals, rhs))),
        Op::Compare {
            out,
            predicate,
            lhs,
            rhs,
        } => (out, data(locals, lhs).compare(predicate, data(locals, rhs))),
        Op::Shift {
            out,
            op,
            value,
            count,
            rule,
        } => (
            out,
            data(locals, value).shift(op, data(locals, count).bits, rule),
        ),
        Op::Select {
            out,
            condition,
            yes,
            no,
        } => {
            let selected = if data(locals, condition).bits == 0 {
                no
            } else {
                yes
            };
            (out, data(locals, selected))
        }
        Op::IntToAddress { out, value } => (
            out,
            Value::new(Type::GuestAddress, data(locals, value).bits),
        ),
        Op::AddressToInt { out, value } => (out, Value::new(Type::I64, data(locals, value).bits)),
        Op::IntCast { out, value, ty } => (out, Value::new(ty, data(locals, value).bits)),
        Op::CheckTarget {
            address,
            when,
            constraint,
        } => {
            if when.is_none_or(|id| data(locals, id).bits != 0) {
                let address = GuestAddress(data(locals, address).bits as u64);
                let valid = match constraint {
                    AddressConstraint::Canonical48 => address.is_canonical_48(),
                    _ => return Err(EvalError::Backend(InterpreterError::UnsupportedIr)),
                };
                if !valid {
                    return Err(fault(
                        pc,
                        thread,
                        FaultClass::NonCanonicalAddress,
                        address,
                        AccessKind::Execute,
                    ));
                }
            }
            return Ok(());
        }
        Op::ReadRegister { out, register } => (out, Value::read(&thread.cpu, register)),
        Op::WriteRegister { register, value } => {
            data(locals, value).write(&mut thread.cpu, register);
            return Ok(());
        }
        Op::Guard { out, address, .. } => {
            locals.insert(
                out,
                Local::Guard(GuestAddress(data(locals, address).bits as u64)),
            );
            return Ok(());
        }
        Op::Load { out, guard, layout } => {
            let address = guarded_address(locals, guard);
            let width = check_access(pc, thread, address, AccessKind::Read, layout)?;
            let mut bytes = [0; 16];
            let value = match layout.endian {
                Endian::Little => {
                    memory
                        .read(address, &mut bytes[..width])
                        .map_err(|err| memory_error(pc, thread, AccessKind::Read, err))?;
                    u128::from_le_bytes(bytes)
                }
                Endian::Big => {
                    memory
                        .read(address, &mut bytes[16 - width..])
                        .map_err(|err| memory_error(pc, thread, AccessKind::Read, err))?;
                    u128::from_be_bytes(bytes)
                }
            };
            (out, Value::new(layout.ty, value))
        }
        Op::Store {
            guard,
            value,
            layout,
        } => {
            let address = guarded_address(locals, guard);
            let width = check_access(pc, thread, address, AccessKind::Write, layout)?;
            let bits = data(locals, value).bits;
            let (bytes, offset) = match layout.endian {
                Endian::Little => (bits.to_le_bytes(), 0),
                Endian::Big => (bits.to_be_bytes(), 16 - width),
            };
            return memory
                .write(address, &bytes[offset..offset + width])
                .map_err(|err| memory_error(pc, thread, AccessKind::Write, err));
        }
        Op::LoadPair64 {
            first,
            second,
            guard,
        } => {
            let address = guarded_address(locals, guard);
            check_access(
                pc,
                thread,
                address,
                AccessKind::Read,
                MemoryLayout::I64_PAIR_LE,
            )?;
            let mut bytes = [0; 16];
            memory
                .read(address, &mut bytes)
                .map_err(|err| memory_error(pc, thread, AccessKind::Read, err))?;
            let pair = u128::from_le_bytes(bytes);
            locals.insert(first, Local::Data(Value::new(Type::I64, pair)));
            (second, Value::new(Type::I64, pair >> 64))
        }
        Op::StorePair64 {
            first,
            second,
            guard,
        } => {
            let address = guarded_address(locals, guard);
            check_access(
                pc,
                thread,
                address,
                AccessKind::Write,
                MemoryLayout::I64_PAIR_LE,
            )?;
            let pair = data(locals, first).bits | (data(locals, second).bits << 64);
            return memory
                .write(address, &pair.to_le_bytes())
                .map_err(|err| memory_error(pc, thread, AccessKind::Write, err));
        }
        _ => return Err(EvalError::Backend(InterpreterError::UnsupportedIr)),
    };
    locals.insert(out, Local::Data(value));
    Ok(())
}

fn check_access(
    pc: GuestAddress,
    thread: &GuestThread,
    address: GuestAddress,
    kind: AccessKind,
    layout: MemoryLayout,
) -> Result<usize, EvalError> {
    let width = std::num::NonZeroU8::new(layout.ty.storage_bytes().expect("verified memory type"))
        .expect("verified width is nonzero");
    let alignment =
        std::num::NonZeroU8::new(layout.alignment).expect("verified alignment is nonzero");
    bifrost_core::validate_guest_access(thread.cpu.isa(), address, width, alignment)
        .map_err(|(address, class)| fault(pc, thread, class, address, kind))?;
    Ok(usize::from(width.get()))
}

fn memory_error(
    pc: GuestAddress,
    thread: &GuestThread,
    kind: AccessKind,
    error: MemoryError,
) -> EvalError {
    match error.guest_fault(pc, thread.id, kind) {
        Some(fault) => EvalError::Guest(fault),
        None => EvalError::Backend(InterpreterError::Memory(error)),
    }
}

fn fault(
    pc: GuestAddress,
    thread: &GuestThread,
    class: FaultClass,
    address: GuestAddress,
    access: AccessKind,
) -> EvalError {
    EvalError::Guest(GuestFault {
        pc,
        thread: thread.id,
        class,
        memory_access: Some((address, access)),
    })
}
