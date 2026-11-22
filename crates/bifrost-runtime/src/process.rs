//! Bounded executable-image sessions.
use crate::{
    RuntimeProfile,
    raw::{ExecutionError, ExitReason, RawInterpreter},
};
use bifrost_core::{
    AbiId, CpuState, ExecMode, GuestFault, GuestIsa, GuestMemory, GuestOs, GuestTarget,
    GuestThread, GuestThreadId, TraceEvent, TraceSink, X86State,
};
use bifrost_loader::LoadError;
pub use bifrost_loader::{Format, Metadata};
#[cfg(feature = "linux-elf")]
pub use bifrost_os_linux::{Io, Vfs};
#[cfg(feature = "linux-elf")]
use bifrost_os_linux::{Linux, SyscallExit, UnsupportedSyscall};
#[cfg(feature = "windows-pe")]
use bifrost_os_windows::{ApiAction, RETURN_PC, STUB_BASE, Windows};
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
    AlreadyRun,
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
                "guest thread {} {:?} fault at {:#x} (memory access {:?})",
                fault.thread.0, fault.class, fault.pc.0, fault.memory_access
            ),
            Self::Budget => f.write_str("guest instruction budget exhausted"),
            Self::AlreadyRun => f.write_str("prepared guest process already ran"),
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
    #[cfg(feature = "macho-inspect")]
    if bytes.starts_with(crate::macho::MAGIC) {
        return crate::macho::inspect(bytes);
    }
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
    run_windows_traced(bytes, io, policy, options, &mut ())
}

#[cfg(feature = "windows-pe")]
pub fn run_windows_traced(
    bytes: &[u8],
    io: WindowsIo<'_>,
    policy: ApiPolicy,
    options: RunOptions,
    trace: &mut dyn TraceSink,
) -> Result<u32, RunError> {
    RuntimeProfile::default()
        .prepare_windows(bytes, io, policy, options)?
        .run(trace)
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
    run_linux_traced(bytes, args, environment, io, vfs, options, &mut ())
}

#[cfg(feature = "linux-elf")]
pub fn run_linux_traced<'a>(
    bytes: &[u8],
    args: &[&[u8]],
    environment: &[&[u8]],
    io: Io<'a>,
    vfs: &'a Vfs,
    options: RunOptions,
    trace: &mut dyn TraceSink,
) -> Result<u32, RunError> {
    RuntimeProfile::default()
        .prepare_linux(bytes, args, environment, io, vfs, options)?
        .run(trace)
}

impl RuntimeProfile {
    /// Selects the backend and maps a static Linux image without executing guest code.
    #[cfg(feature = "linux-elf")]
    pub fn prepare_linux<'a>(
        &self,
        bytes: &[u8],
        args: &[&[u8]],
        environment: &[&[u8]],
        io: Io<'a>,
        vfs: &'a Vfs,
        options: RunOptions,
    ) -> Result<PreparedProcess<'a>, RunError> {
        let elf = bifrost_loader_elf::parse(bytes)?;
        let mode = self
            .select_execution(bare_target(), options.execution)
            .map_err(RunError::Mode)?;
        let (memory, state) = elf.start(args, environment)?;
        let services = Services::Linux(Linux::new(io, vfs));
        Ok(PreparedProcess::new(
            elf.image.metadata,
            memory,
            state,
            services,
            mode,
            options.instruction_limit,
        ))
    }

    /// Resolves all PE imports, selects the provider policy, and maps the image without running it.
    #[cfg(feature = "windows-pe")]
    pub fn prepare_windows<'a>(
        &self,
        bytes: &[u8],
        io: WindowsIo<'a>,
        policy: ApiPolicy,
        options: RunOptions,
    ) -> Result<PreparedProcess<'a>, RunError> {
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
        let mode = self
            .select_execution(bare_target(), options.execution)
            .map_err(RunError::Mode)?;
        let services =
            Services::Windows(Windows::new(io, policy, &targets).map_err(RunError::Windows)?);
        let (mut memory, state) = pe.start(&targets, RETURN_PC)?;
        memory
            .map(
                bifrost_core::GuestAddress(STUB_BASE),
                bifrost_loader::PAGE,
                bifrost_core::Protection::Guard,
            )
            .map_err(LoadError::from)?;
        Ok(PreparedProcess::new(
            pe.image.metadata,
            memory,
            state,
            services,
            mode,
            options.instruction_limit,
        ))
    }
}

/// One executable session with owned guest state and borrowed host I/O.
/// A session runs once. Its thread snapshot remains accessible at exit or on error.
pub struct PreparedProcess<'a> {
    metadata: Metadata,
    memory: bifrost_memory::AddressSpace,
    thread: GuestThread,
    services: Services<'a>,
    mode: ExecMode,
    limit: u64,
    started: bool,
}

impl<'a> PreparedProcess<'a> {
    fn new(
        metadata: Metadata,
        memory: bifrost_memory::AddressSpace,
        state: X86State,
        services: Services<'a>,
        mode: ExecMode,
        limit: u64,
    ) -> Self {
        Self {
            metadata,
            memory,
            services,
            mode,
            limit,
            started: false,
            thread: GuestThread {
                id: GuestThreadId(1),
                cpu: CpuState::X86_64(Box::new(state)),
                pending_fault: None,
            },
        }
    }

    pub const fn metadata(&self) -> Metadata {
        self.metadata
    }

    pub const fn execution(&self) -> ExecMode {
        self.mode
    }

    pub const fn thread(&self) -> &GuestThread {
        &self.thread
    }

    pub fn run(&mut self, trace: &mut dyn TraceSink) -> Result<u32, RunError> {
        if std::mem::replace(&mut self.started, true) {
            return Err(RunError::AlreadyRun);
        }
        let Self {
            memory,
            thread,
            services,
            mode,
            limit,
            ..
        } = self;
        execute(memory, thread, services, *mode, *limit, trace)
    }
}

enum Services<'a> {
    #[cfg(feature = "linux-elf")]
    Linux(Linux<'a>),
    #[cfg(feature = "windows-pe")]
    Windows(Windows<'a>),
}

impl Services<'_> {
    fn dispatch(
        &mut self,
        thread: &mut GuestThread,
        memory: &mut dyn GuestMemory,
    ) -> Result<Action, RunError> {
        match self {
            #[cfg(feature = "linux-elf")]
            Self::Linux(linux) => {
                let CpuState::X86_64(cpu) = &mut thread.cpu else {
                    return Err(RunError::Backend);
                };
                let mut opcode = [0; 2];
                if memory.fetch(cpu.rip, &mut opcode).is_err() || opcode != [0x0f, 0x05] {
                    return Ok(Action::Step);
                }
                Ok(
                    match linux.syscall(cpu, memory).map_err(RunError::Syscall)? {
                        SyscallExit::Continue => Action::Continue,
                        SyscallExit::Exit(status) => Action::Exit(status.into()),
                    },
                )
            }
            #[cfg(feature = "windows-pe")]
            Self::Windows(windows) => Ok(
                match windows
                    .dispatch(&mut thread.cpu, memory)
                    .map_err(RunError::Windows)?
                {
                    ApiAction::Step => Action::Step,
                    ApiAction::Returned => Action::Continue,
                    ApiAction::Exit(status) => Action::Exit(status),
                },
            ),
        }
    }
}

enum Action {
    Step,
    Continue,
    Exit(u32),
}

fn bare_target() -> GuestTarget {
    GuestTarget::new(
        GuestIsa::X86_64,
        GuestOs::None,
        AbiId::Bare,
        Default::default(),
    )
    .expect("bare execution target")
}

fn execute(
    memory: &mut bifrost_memory::AddressSpace,
    thread: &mut GuestThread,
    services: &mut Services<'_>,
    mode: ExecMode,
    limit: u64,
    trace: &mut dyn TraceSink,
) -> Result<u32, RunError> {
    let target = bare_target();
    let mut engine = match mode {
        ExecMode::Interpreter => Engine::Interpreter(RawInterpreter::new(&target, memory, trace)?),
        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        ExecMode::Jit => Engine::Jit(Box::new(crate::raw::RawJit::new(
            &target,
            memory,
            trace,
            crate::raw::SystemJitMemory,
        )?)),
        _ => return Err(RunError::Backend),
    };
    for _ in 0..limit {
        let pc = thread.cpu.pc();
        let action = services.dispatch(thread, engine.memory());
        if !matches!(&action, Ok(Action::Step)) {
            engine.trace().record(TraceEvent::Step {
                thread: thread.id,
                pc,
            });
        }
        match action? {
            Action::Exit(status) => return Ok(status),
            Action::Continue => continue,
            Action::Step => {}
        }
        match engine.step(thread)? {
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
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    Jit(Box<crate::raw::RawJit<'a, crate::raw::SystemJitMemory>>),
}

impl Engine<'_> {
    fn trace(&mut self) -> &mut dyn TraceSink {
        match self {
            Self::Interpreter(engine) => engine.trace_mut(),
            #[cfg(all(
                feature = "jit-cranelift",
                any(target_arch = "x86_64", target_arch = "aarch64"),
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Jit(engine) => engine.trace_mut(),
        }
    }

    fn memory(&mut self) -> &mut dyn GuestMemory {
        match self {
            Self::Interpreter(engine) => engine.memory_mut(),
            #[cfg(all(
                feature = "jit-cranelift",
                any(target_arch = "x86_64", target_arch = "aarch64"),
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Jit(engine) => engine.memory_mut(),
        }
    }

    fn step(&mut self, thread: &mut GuestThread) -> Result<ExitReason, ExecutionError> {
        match self {
            Self::Interpreter(engine) => engine.step(thread),
            #[cfg(all(
                feature = "jit-cranelift",
                any(target_arch = "x86_64", target_arch = "aarch64"),
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Jit(engine) => engine.step(thread),
        }
    }
}
