use crate::{MachineCode, lower};
use bifrost_core::{
    AccessKind, CpuState, FaultClass, GuestAddress, GuestFault, GuestIsa, GuestMemory, GuestThread,
    GuestThreadId, MemoryError,
};
use bifrost_host::jit::{ExecutableImage, JitMemoryManager};
use bifrost_ir::CodeSpan;
use std::{
    any::Any,
    ffi::c_void,
    fmt, io,
    mem::offset_of,
    num::NonZeroU8,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
};

#[repr(C)]
struct Frame {
    general: [u64; 32],
    flags: u64,
    next_pc: u64,
    detail: u64,
    helper_value: u64,
    helper_high: u64,
    tpidr: u64,
    tpidrro: u64,
    vectors: u64,
    helper_vectors: [u8; 32],
}

const _: () = {
    assert!(offset_of!(Frame, flags) == lower::FLAGS as usize);
    assert!(offset_of!(Frame, next_pc) == lower::NEXT_PC as usize);
    assert!(offset_of!(Frame, detail) == lower::FAULT_DETAIL as usize);
    assert!(offset_of!(Frame, helper_value) == lower::HELPER_VALUE as usize);
    assert!(offset_of!(Frame, helper_high) == lower::HELPER_HIGH as usize);
    assert!(offset_of!(Frame, tpidr) == lower::TPIDR as usize);
    assert!(offset_of!(Frame, tpidrro) == lower::TPIDRRO as usize);
    assert!(offset_of!(Frame, vectors) == lower::VECTOR_REGISTERS as usize);
    assert!(offset_of!(Frame, helper_vectors) == lower::HELPER_VECTORS as usize);
};

type Helper = unsafe extern "C" fn(*mut Frame, *mut c_void, u64, u64, u32) -> u32;
type Entry = unsafe extern "C" fn(*mut Frame, *mut c_void, Helper) -> u32;

pub struct PublishedBlock<Image> {
    image: Image,
    guest: CodeSpan,
    guest_isa: GuestIsa,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockExit {
    Dispatch(GuestAddress),
    Fault(GuestFault),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ExecutionError {
    WrongGuestIsa,
    WrongGuestPc {
        expected: GuestAddress,
        actual: GuestAddress,
    },
    Memory(MemoryError),
    InvalidExit,
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongGuestIsa => f.write_str("guest ISA does not match the compiled block"),
            Self::WrongGuestPc { expected, actual } => write!(
                f,
                "JIT block at {:#x} cannot run at {:#x}",
                expected.0, actual.0
            ),
            Self::Memory(error) => error.fmt(f),
            Self::InvalidExit => f.write_str("invalid machine-code exit"),
        }
    }
}
impl std::error::Error for ExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Memory(error) => Some(error),
            _ => None,
        }
    }
}

impl MachineCode {
    pub fn publish<M: JitMemoryManager>(
        &self,
        manager: &M,
    ) -> io::Result<PublishedBlock<M::Image>> {
        Ok(PublishedBlock {
            image: manager.publish(&self.bytes)?,
            guest: self.guest,
            guest_isa: self.guest_isa,
        })
    }
}

impl<Image: ExecutableImage> PublishedBlock<Image> {
    /// Enters only code emitted by this crate, supplying a live frame and runtime helper.
    pub fn run(
        &self,
        thread: &mut GuestThread,
        memory: &mut dyn GuestMemory,
    ) -> Result<BlockExit, ExecutionError> {
        if thread.cpu.isa() != self.guest_isa {
            return Err(ExecutionError::WrongGuestIsa);
        }
        if let Some(fault) = thread.pending_fault {
            return Ok(BlockExit::Fault(fault));
        }
        let pc = thread.cpu.pc();
        if pc != self.guest.start {
            return Err(ExecutionError::WrongGuestPc {
                expected: self.guest.start,
                actual: pc,
            });
        }
        let mut frame = Frame {
            general: [0; 32],
            flags: 0,
            next_pc: pc.0,
            detail: 0,
            helper_value: 0,
            helper_high: 0,
            tpidr: 0,
            tpidrro: 0,
            vectors: 0,
            helper_vectors: [0; 32],
        };
        match &mut thread.cpu {
            CpuState::X86_64(state) => {
                frame.general[..16].copy_from_slice(&state.gpr);
                frame.flags = state.rflags;
            }
            CpuState::AArch64(state) => {
                frame.general[..31].copy_from_slice(&state.x);
                frame.general[31] = state.sp.0;
                frame.flags = u64::from(state.nzcv);
                frame.tpidr = state.tpidr_el0.0;
                frame.tpidrro = state.tpidrro_el0.0;
                frame.vectors = state.v.as_mut_ptr() as usize as u64;
            }
            _ => return Err(ExecutionError::WrongGuestIsa),
        }
        let mut context = MemoryContext {
            memory,
            isa: self.guest_isa,
            pc,
            thread: thread.id,
            failure: None,
        };
        // SAFETY: Only this crate can construct MachineCode. The unsafe manager contract ensures
        // its executable entry is exactly the generated code and lives for the entire call.
        // Both ABI arguments and the optional AArch64 vector pointer refer to live,
        // uniquely borrowed storage for this call only.
        let status = unsafe {
            let entry: Entry = std::mem::transmute(self.image.entry().as_ptr());
            entry(
                &mut frame,
                (&mut context as *mut MemoryContext<'_>).cast(),
                run_helper,
            )
        };
        match &mut thread.cpu {
            CpuState::X86_64(state) => {
                state.gpr.copy_from_slice(&frame.general[..16]);
                state.rflags = frame.flags;
            }
            CpuState::AArch64(state) => {
                state.x.copy_from_slice(&frame.general[..31]);
                state.sp = GuestAddress(frame.general[31]);
                state.nzcv = frame.flags as u32;
                state.tpidr_el0 = GuestAddress(frame.tpidr);
            }
            _ => return Err(ExecutionError::WrongGuestIsa),
        }
        let (class, access) = match i64::from(status) {
            lower::DISPATCH => {
                let next = GuestAddress(frame.next_pc);
                thread.cpu.set_pc(next);
                return Ok(BlockExit::Dispatch(next));
            }
            lower::NONCANONICAL => (
                FaultClass::NonCanonicalAddress,
                Some((GuestAddress(frame.detail), AccessKind::Execute)),
            ),
            lower::ILLEGAL => (FaultClass::IllegalInstruction, None),
            lower::TRAP => (FaultClass::Trap(frame.detail as u32), None),
            lower::ALIGNMENT_READ => (
                FaultClass::Alignment,
                Some((GuestAddress(frame.detail), AccessKind::Read)),
            ),
            lower::ALIGNMENT_WRITE => (
                FaultClass::Alignment,
                Some((GuestAddress(frame.detail), AccessKind::Write)),
            ),
            lower::HELPER_FAILED => match context.failure {
                Some(HelperError::Guest(fault)) => (fault.class, fault.memory_access),
                Some(HelperError::Memory(error)) => return Err(ExecutionError::Memory(error)),
                Some(HelperError::Panic(payload)) => resume_unwind(payload),
                None => return Err(ExecutionError::InvalidExit),
            },
            _ => return Err(ExecutionError::InvalidExit),
        };
        let fault = GuestFault {
            pc,
            thread: thread.id,
            class,
            memory_access: access,
        };
        thread.pending_fault = Some(fault);
        Ok(BlockExit::Fault(fault))
    }
}

struct MemoryContext<'a> {
    memory: &'a mut dyn GuestMemory,
    isa: GuestIsa,
    pc: GuestAddress,
    thread: GuestThreadId,
    failure: Option<HelperError>,
}

enum HelperError {
    Guest(GuestFault),
    Memory(MemoryError),
    Panic(Box<dyn Any + Send>),
}

unsafe extern "C" fn run_helper(
    frame: *mut Frame,
    context: *mut c_void,
    address: u64,
    value: u64,
    options: u32,
) -> u32 {
    // SAFETY: This callback is private and is only passed to code generated by this crate.
    // That code forwards the live pointers without modification.
    let (frame, context) = unsafe { (&mut *frame, &mut *context.cast::<MemoryContext<'_>>()) };
    match options {
        lower::DIV_UNSIGNED => {
            frame.helper_value = address.checked_div(value).unwrap_or(0);
            return 0;
        }
        lower::DIV_SIGNED => {
            frame.helper_value = if value == 0 {
                0
            } else {
                (address as i64).wrapping_div(value as i64) as u64
            };
            return 0;
        }
        _ => {}
    }
    let call = catch_unwind(AssertUnwindSafe(|| {
        let value = u128::from(value) | (u128::from(frame.helper_high) << 64);
        context.access(
            GuestAddress(address),
            value,
            options,
            &mut frame.helper_vectors,
        )
    }));
    match call {
        Ok(Ok(value)) => {
            frame.helper_value = value as u64;
            frame.helper_high = (value >> 64) as u64;
            0
        }
        Ok(Err(error)) => {
            context.failure = Some(error);
            1
        }
        Err(payload) => {
            context.failure = Some(HelperError::Panic(payload));
            1
        }
    }
}

impl MemoryContext<'_> {
    fn access(
        &mut self,
        address: GuestAddress,
        value: u128,
        options: u32,
        vectors: &mut [u8; 32],
    ) -> Result<u128, HelperError> {
        let width = NonZeroU8::new(options as u8).expect("generated width is nonzero");
        let alignment =
            NonZeroU8::new((options >> 8) as u8).expect("generated alignment is nonzero");
        let big_endian = options & (1 << 16) != 0;
        let write = options & lower::MEMORY_WRITE != 0;
        let access = if write {
            AccessKind::Write
        } else {
            AccessKind::Read
        };
        bifrost_core::validate_guest_access(self.isa, address, width, alignment).map_err(
            |(address, class)| {
                HelperError::Guest(GuestFault {
                    pc: self.pc,
                    thread: self.thread,
                    class,
                    memory_access: Some((address, access)),
                })
            },
        )?;

        let width = usize::from(width.get());
        if width == vectors.len() {
            let result = if write {
                self.memory.write(address, vectors)
            } else {
                self.memory.read(address, vectors)
            };
            return result
                .map(|()| 0)
                .map_err(|error| self.access_error(access, error));
        }
        let start = if big_endian { 16 - width } else { 0 };
        let range = start..start + width;
        let result = if options & lower::MEMORY_COMPARE_EXCHANGE != 0 {
            self.memory
                .compare_exchange_le(address, width, value as u64, (value >> 64) as u64)
                .map(u128::from)
        } else if options & lower::MEMORY_ATOMIC_ADD != 0 {
            self.memory
                .fetch_add_le(address, width, value as u64)
                .map(u128::from)
        } else if write {
            let bytes = if big_endian {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            };
            self.memory.write(address, &bytes[range]).map(|()| 0)
        } else {
            let mut bytes = [0; 16];
            self.memory.read(address, &mut bytes[range]).map(|()| {
                if big_endian {
                    u128::from_be_bytes(bytes)
                } else {
                    u128::from_le_bytes(bytes)
                }
            })
        };
        result.map_err(|error| self.access_error(access, error))
    }

    fn access_error(&self, access: AccessKind, error: MemoryError) -> HelperError {
        match error.guest_fault(self.pc, self.thread, access) {
            Some(fault) => HelperError::Guest(fault),
            None => HelperError::Memory(error),
        }
    }
}
