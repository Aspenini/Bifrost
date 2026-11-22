//! Bounded executable-image sessions.
use crate::raw::{ExecutionError, ExitReason, RawInterpreter};
use bifrost_core::{
    AbiId, CpuState, ExecMode, GuestFault, GuestIsa, GuestMemory, GuestOs, GuestTarget,
    GuestThread, GuestThreadId,
};
use bifrost_loader::LoadError;
pub use bifrost_loader::{Format, Metadata};
#[cfg(feature = "linux-elf")]
pub use bifrost_os_linux::{Io, Vfs};
#[cfg(feature = "linux-elf")]
use bifrost_os_linux::{Linux, SyscallExit, UnsupportedSyscall};
#[cfg(feature = "windows-pe")]
pub use bifrost_os_windows::{ApiPolicy, Io as WindowsIo};
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
    #[cfg(feature = "linux-elf")]
    Syscall(UnsupportedSyscall),
    #[cfg(feature = "windows-pe")]
    Windows(bifrost_os_windows::ApiError),
    #[cfg(feature = "windows-pe")]
    Import {
        module: String,
        symbol: String,
    },
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
            #[cfg(feature = "linux-elf")]
            Self::Syscall(error) => error.fmt(f),
            #[cfg(feature = "windows-pe")]
            Self::Windows(error) => error.fmt(f),
            #[cfg(feature = "windows-pe")]
            Self::Import { module, symbol } => write!(f, "unsupported PE import {module}!{symbol}"),
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
    #[cfg(feature = "linux-elf")]
    if bytes.starts_with(b"\x7fELF") {
        return Ok(bifrost_loader_elf::parse(bytes)?.image.metadata);
    }
    #[cfg(feature = "windows-pe")]
    if bytes.starts_with(b"MZ") {
        return Ok(bifrost_loader_pe::parse(bytes)?.image.metadata);
    }
    Err(LoadError::Unsupported(
        "unknown or disabled executable format",
    ))
}

#[cfg(feature = "windows-pe")]
pub fn inspect_pe(bytes: &[u8]) -> Result<bifrost_loader_pe::Pe<'_>, LoadError> {
    bifrost_loader_pe::parse(bytes)
}

#[cfg(feature = "windows-pe")]
pub fn run_windows(
    bytes: &[u8],
    io: WindowsIo<'_>,
    policy: ApiPolicy,
    options: RunOptions,
) -> Result<u32, RunError> {
    use bifrost_os_windows::{ApiAction, RETURN_PC, STUB_BASE, Windows};
    let pe = bifrost_loader_pe::parse(bytes)?;
    let targets = pe
        .imports
        .iter()
        .map(|import| {
            bifrost_os_windows::resolve(import.module, import.symbol).ok_or_else(|| {
                RunError::Import {
                    module: import.module.into(),
                    symbol: import.symbol.into(),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (mut memory, state) = pe.start(&targets, RETURN_PC)?;
    memory
        .map(
            bifrost_core::GuestAddress(STUB_BASE),
            bifrost_loader::PAGE,
            bifrost_core::Protection::Guard,
        )
        .map_err(LoadError::from)?;
    let mut windows = Windows::new(io, policy, &targets).map_err(RunError::Windows)?;
    execute(&mut memory, state, options, |thread, memory| {
        Ok(
            match windows
                .dispatch(&mut thread.cpu, memory)
                .map_err(RunError::Windows)?
            {
                ApiAction::Step => Action::Step,
                ApiAction::Returned => Action::Continue,
                ApiAction::Exit(status) => Action::Exit(status),
            },
        )
    })
}

#[cfg(feature = "linux-elf")]
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
