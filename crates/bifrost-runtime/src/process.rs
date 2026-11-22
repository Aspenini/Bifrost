//! Bounded executable-image sessions.
use crate::raw::{ExecutionError, ExitReason, RawInterpreter};
use bifrost_core::{
    AbiId, CpuState, ExecMode, GuestFault, GuestIsa, GuestMemory, GuestOs, GuestTarget,
    GuestThread, GuestThreadId,
};
use bifrost_loader::LoadError;
pub use bifrost_loader::{Format, Metadata};
pub use bifrost_os_linux::{Io, Vfs};
use bifrost_os_linux::{Linux, SyscallExit, UnsupportedSyscall};
use std::fmt;

#[derive(Clone, Copy, Debug)]
pub struct RunOptions {
    pub execution: ExecMode,
    pub instruction_limit: u64,
}
impl Default for RunOptions {
    fn default() -> Self {
        Self {
            execution: ExecMode::Auto,
            instruction_limit: 1_000_000,
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum RunError {
    Load(LoadError),
    Execution(ExecutionError),
    Syscall(UnsupportedSyscall),
    Mode(crate::UnsupportedExecutionMode),
    Fault(GuestFault),
    Budget,
    Backend,
}
impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(error) => error.fmt(f),
            Self::Execution(error) => error.fmt(f),
            Self::Syscall(error) => error.fmt(f),
            Self::Mode(error) => error.fmt(f),
            Self::Fault(fault) => write!(
                f,
                "guest {:?} fault at {:#x} ({:?})",
                fault.class, fault.pc.0, fault.memory_access
            ),
            Self::Budget => f.write_str("guest instruction budget exhausted"),
            Self::Backend => f.write_str("invalid process execution backend"),
        }
    }
}
impl std::error::Error for RunError {}
impl From<LoadError> for RunError {
    fn from(error: LoadError) -> Self {
        Self::Load(error)
    }
}
impl From<ExecutionError> for RunError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}

pub fn inspect(bytes: &[u8]) -> Result<Metadata, LoadError> {
    Ok(bifrost_loader_elf::parse(bytes)?.image.metadata)
}

pub fn run_linux<'a>(
    bytes: &[u8],
    args: &[&[u8]],
    environment: &[&[u8]],
    io: Io<'a>,
    vfs: &'a Vfs,
    options: RunOptions,
) -> Result<u32, RunError> {
    let (mut memory, state) = bifrost_loader_elf::parse(bytes)?.start(args, environment)?;
    let mut linux = Linux::new(io, vfs);
    execute(&mut memory, state, options, |thread, memory| {
        let CpuState::X86_64(cpu) = &mut thread.cpu else {
            return Err(RunError::Backend);
        };
        let mut opcode = [0; 2];
        if memory.fetch(cpu.rip, &mut opcode).is_ok() && opcode == [0x0f, 0x05] {
            return match linux.syscall(cpu, memory).map_err(RunError::Syscall)? {
                SyscallExit::Continue => Ok(Action::Continue),
                SyscallExit::Exit(status) => Ok(Action::Exit(status.into())),
            };
        }
        Ok(Action::Step)
    })
}

enum Action {
    Step,
    Continue,
    Exit(u32),
}

fn execute(
    memory: &mut bifrost_memory::AddressSpace,
    state: bifrost_core::X86State,
    options: RunOptions,
    mut boundary: impl FnMut(&mut GuestThread, &mut dyn GuestMemory) -> Result<Action, RunError>,
) -> Result<u32, RunError> {
    let target = GuestTarget::new(
        GuestIsa::X86_64,
        GuestOs::None,
        AbiId::Bare,
        Default::default(),
    )
    .expect("bare execution target");
    let mode = crate::RuntimeProfile::default()
        .select_execution(target.clone(), options.execution)
        .map_err(RunError::Mode)?;
    let mut trace = ();
    let mut thread = GuestThread {
        id: GuestThreadId(1),
        cpu: CpuState::X86_64(Box::new(state)),
        pending_fault: None,
    };
    let mut engine = match mode {
        ExecMode::Interpreter => {
            Engine::Interpreter(RawInterpreter::new(&target, memory, &mut trace)?)
        }
        #[cfg(all(
            feature = "jit-cranelift",
            any(
                all(
                    target_arch = "x86_64",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(target_arch = "aarch64", target_os = "linux")
            )
        ))]
        ExecMode::Jit => Engine::Jit(Box::new(crate::raw::RawJit::new(
            &target,
            memory,
            &mut trace,
            crate::raw::SystemJitMemory,
        )?)),
        _ => return Err(RunError::Backend),
    };
    for _ in 0..options.instruction_limit {
        match boundary(&mut thread, engine.memory())? {
            Action::Exit(status) => return Ok(status),
            Action::Continue => continue,
            Action::Step => {}
        }
        match engine.step(&mut thread)? {
            ExitReason::Dispatch(_) => {}
            ExitReason::Fault(fault) => return Err(RunError::Fault(fault)),
            _ => return Err(RunError::Backend),
        }
    }
    Err(RunError::Budget)
}

enum Engine<'a> {
    Interpreter(RawInterpreter<'a>),
    #[cfg(all(
        feature = "jit-cranelift",
        any(
            all(
                target_arch = "x86_64",
                any(target_os = "windows", target_os = "linux")
            ),
            all(target_arch = "aarch64", target_os = "linux")
        )
    ))]
    Jit(Box<crate::raw::RawJit<'a, crate::raw::SystemJitMemory>>),
}

impl Engine<'_> {
    fn memory(&mut self) -> &mut dyn GuestMemory {
        match self {
            Self::Interpreter(engine) => engine.memory_mut(),
            #[cfg(all(
                feature = "jit-cranelift",
                any(
                    all(
                        target_arch = "x86_64",
                        any(target_os = "windows", target_os = "linux")
                    ),
                    all(target_arch = "aarch64", target_os = "linux")
                )
            ))]
            Self::Jit(engine) => engine.memory_mut(),
        }
    }

    fn step(&mut self, thread: &mut GuestThread) -> Result<ExitReason, ExecutionError> {
        match self {
            Self::Interpreter(engine) => engine.step(thread),
            #[cfg(all(
                feature = "jit-cranelift",
                any(
                    all(
                        target_arch = "x86_64",
                        any(target_os = "windows", target_os = "linux")
                    ),
                    all(target_arch = "aarch64", target_os = "linux")
                )
            ))]
            Self::Jit(engine) => engine.step(thread),
        }
    }
}
