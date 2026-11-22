use super::{MAX_STEPS, Session, Status};
use bifrost_runtime::{
    core::{
        AbiId, Arm64State, CpuState, GuestAddress, GuestIsa, GuestOs, GuestTarget, GuestThread,
        GuestThreadId, X86State,
    },
    memory::{AddressSpace, GuestMemory, GuestMemoryRead, MemoryError, Protection},
    raw::{ExitReason, RawInterpreter},
};
use std::{panic::catch_unwind, slice};

const MAX_CODE: usize = 1024 * 1024;
const PAGE: usize = 4096;
const CODE: GuestAddress = GuestAddress(0x10000);
const STACK_TOP: u64 = 0x200000;
const STACK_PAGES: usize = 16;

#[repr(C)]
#[derive(Default)]
pub struct RawResult {
    pub pc: u64,
    pub return_register: u64,
    pub instructions: u64,
}

/// Interprets raw scalar code and stops when the guest PC reaches the end of the supplied bytes.
///
/// # Safety
/// The session remains live, code is readable for the stated size, and result points to
/// aligned writable storage disjoint from the code and session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bifrost_run_raw(
    session: *const Session,
    isa: u32,
    code: *const u8,
    len: usize,
    limit: u64,
    result: *mut RawResult,
) -> Status {
    let isa = match isa {
        1 => GuestIsa::X86_64,
        2 if len.is_multiple_of(4) => GuestIsa::AArch64,
        _ => return Status::InvalidArgument,
    };
    if session.is_null()
        || code.is_null()
        || !(1..=MAX_CODE).contains(&len)
        || !(1..=MAX_STEPS).contains(&limit)
        || result.is_null()
    {
        return Status::InvalidArgument;
    }
    catch_unwind(|| {
        // SAFETY: The caller guarantees both ranges and their disjointness.
        let code = unsafe { slice::from_raw_parts(code, len) };
        let result = unsafe {
            result.write(RawResult::default());
            &mut *result
        };
        run(isa, code, limit, result)
    })
    .unwrap_or(Status::InternalError)
}

fn run(isa: GuestIsa, code: &[u8], limit: u64, result: &mut RawResult) -> Status {
    let Ok(mut memory) = CodeMemory::new(code) else {
        return Status::LoadFailure;
    };
    let target = GuestTarget::new(isa, GuestOs::None, AbiId::Bare, Default::default())
        .expect("bare CPU target");
    let mut trace = ();
    let end = memory.end;
    let Ok(mut engine) = RawInterpreter::new(&target, &mut memory, &mut trace) else {
        return Status::Unavailable;
    };
    let cpu = match isa {
        GuestIsa::X86_64 => {
            let mut state = X86State {
                rip: CODE,
                ..Default::default()
            };
            state.gpr[4] = STACK_TOP;
            CpuState::X86_64(Box::new(state))
        }
        GuestIsa::AArch64 => CpuState::AArch64(Box::new(Arm64State {
            pc: CODE,
            sp: GuestAddress(STACK_TOP),
            ..Default::default()
        })),
        _ => return Status::Unavailable,
    };
    let mut thread = GuestThread {
        id: GuestThreadId(1),
        cpu,
        pending_fault: None,
    };
    let status = loop {
        let pc = thread.cpu.pc().0;
        if pc == end {
            break Status::Ok;
        }
        if !(CODE.0..end).contains(&pc) {
            break Status::GuestFailure;
        }
        if result.instructions == limit {
            break Status::BudgetExhausted;
        }
        if !matches!(engine.step(&mut thread), Ok(ExitReason::Dispatch(_))) {
            break Status::GuestFailure;
        }
        result.instructions += 1;
    };
    result.pc = thread.cpu.pc().0;
    result.return_register = match thread.cpu {
        CpuState::X86_64(cpu) => cpu.gpr[0],
        CpuState::AArch64(cpu) => cpu.x[0],
        _ => return Status::InternalError,
    };
    status
}

struct CodeMemory {
    space: AddressSpace,
    end: u64,
}

impl CodeMemory {
    fn new(code: &[u8]) -> Result<Self, MemoryError> {
        let pages = code.len().div_ceil(PAGE);
        let mut space = AddressSpace::new(PAGE, pages + STACK_PAGES + 1)?;
        let code_size = (pages * PAGE) as u64;
        let stack_size = (STACK_PAGES * PAGE) as u64;
        let stack = STACK_TOP - stack_size;
        space.map(CODE, code_size, Protection::ReadWrite)?;
        space.write(CODE, code)?;
        space.protect(CODE, code_size, Protection::ReadExecute)?;
        space.map(
            GuestAddress(stack - PAGE as u64),
            PAGE as u64,
            Protection::Guard,
        )?;
        space.map(GuestAddress(stack), stack_size, Protection::ReadWrite)?;
        Ok(Self {
            space,
            end: CODE.0 + code.len() as u64,
        })
    }
}

impl GuestMemoryRead for CodeMemory {
    fn read(&self, address: GuestAddress, output: &mut [u8]) -> Result<(), MemoryError> {
        self.space.read(address, output)
    }

    fn fetch(&self, address: GuestAddress, output: &mut [u8]) -> Result<(), MemoryError> {
        if address.0 >= self.end || address.0 < CODE.0 {
            return Err(MemoryError::Unmapped(address));
        }
        if output.len() as u64 > self.end - address.0 {
            return Err(MemoryError::Unmapped(GuestAddress(self.end)));
        }
        self.space.fetch(address, output)
    }
}

impl GuestMemory for CodeMemory {
    fn write(&mut self, address: GuestAddress, bytes: &[u8]) -> Result<(), MemoryError> {
        self.space.write(address, bytes)
    }

    fn check_write(&self, address: GuestAddress, len: usize) -> Result<(), MemoryError> {
        self.space.check_write(address, len)
    }
}
