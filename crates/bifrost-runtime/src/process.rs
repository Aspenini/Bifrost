//! Bounded executable-image sessions.
use crate::{
    RuntimeProfile,
    raw::{ExecutionError, ExitReason, RawInterpreter},
};
#[cfg(feature = "linux-elf")]
use bifrost_core::GuestMemoryRead;
use bifrost_core::{
    AbiId, CpuState, ExecMode, GuestAddress, GuestFault, GuestIsa, GuestMemory, GuestOs,
    GuestTarget, GuestThread, GuestThreadId, TraceEvent, TraceSink, X86State,
};
use bifrost_loader::LoadError;
pub use bifrost_loader::{Format, Metadata};
#[cfg(feature = "linux-elf")]
pub use bifrost_os_linux::{Io, Vfs};
#[cfg(feature = "linux-elf")]
use bifrost_os_linux::{Linux, ThreadSyscall, UnsupportedSyscall};
#[cfg(feature = "windows-pe")]
use bifrost_os_windows::{ApiAction, RETURN_PC, STUB_BASE, Windows};
#[cfg(feature = "windows-pe")]
pub use bifrost_os_windows::{ApiPolicy, Io as WindowsIo};
use std::{collections::VecDeque, fmt};

const QUANTUM: u64 = 64;
#[cfg(feature = "linux-elf")]
const MAX_THREADS: usize = 64;
#[cfg(feature = "linux-elf")]
const THREAD_STACK_BYTES: u64 = 64 * 1024;
#[cfg(feature = "linux-elf")]
const THREAD_STACK_TOP: u64 = 0x7fff_ff00_0000;

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
    ThreadLimit,
    ThreadEntry(GuestAddress),
    ThreadStack(bifrost_core::MemoryError),
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
            Self::ThreadLimit => f.write_str("guest thread limit reached"),
            Self::ThreadEntry(pc) => write!(f, "guest thread entry {:#x} is not executable", pc.0),
            Self::ThreadStack(error) => write!(f, "could not map guest thread stack: {error}"),
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
    if crate::macho::is_universal(bytes) {
        return Err(LoadError::Unsupported(
            "select a Mach-O slice with inspect_universal",
        ));
    }
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
    threads: Vec<ThreadSlot>,
    services: Services<'a>,
    mode: ExecMode,
    limit: u64,
    started: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadStatus {
    Runnable,
    Exited(u32),
    Stopped,
}

struct ThreadSlot {
    thread: GuestThread,
    status: ThreadStatus,
}

impl ThreadSlot {
    fn new(id: GuestThreadId, state: X86State) -> Self {
        Self {
            thread: GuestThread {
                id,
                cpu: CpuState::X86_64(Box::new(state)),
                pending_fault: None,
            },
            status: ThreadStatus::Runnable,
        }
    }
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
            threads: vec![ThreadSlot::new(GuestThreadId(1), state)],
        }
    }

    pub const fn metadata(&self) -> Metadata {
        self.metadata
    }

    pub const fn execution(&self) -> ExecMode {
        self.mode
    }

    /// Initial guest thread, available before and after execution.
    pub fn thread(&self) -> &GuestThread {
        &self.threads[0].thread
    }

    pub fn threads(&self) -> impl ExactSizeIterator<Item = (&GuestThread, ThreadStatus)> {
        self.threads.iter().map(|slot| (&slot.thread, slot.status))
    }

    /// Registers an additional Linux guest thread before execution.
    /// The entry is a bare x86-64 entry point, with the argument in RDI and an empty stack.
    #[cfg(feature = "linux-elf")]
    pub fn add_linux_thread(
        &mut self,
        entry: GuestAddress,
        argument: u64,
    ) -> Result<GuestThreadId, RunError> {
        if self.started {
            return Err(RunError::AlreadyRun);
        }
        if !matches!(self.services, Services::Linux(_)) {
            return Err(RunError::Load(LoadError::Unsupported(
                "Linux thread enrollment on a non-Linux image",
            )));
        }
        let index = self.threads.len();
        if index >= MAX_THREADS {
            return Err(RunError::ThreadLimit);
        }
        if !entry.is_canonical_48() || self.memory.fetch(entry, &mut [0]).is_err() {
            return Err(RunError::ThreadEntry(entry));
        }
        let slot_size = THREAD_STACK_BYTES + bifrost_loader::PAGE;
        let guard = GuestAddress(THREAD_STACK_TOP - index as u64 * slot_size);
        let stack = self
            .memory
            .map_guarded_stack(guard, THREAD_STACK_BYTES)
            .map_err(RunError::ThreadStack)?;
        let id = GuestThreadId(index as u64 + 1);
        let mut state = X86State {
            rip: entry,
            ..X86State::default()
        };
        state.gpr[4] = stack.0;
        state.gpr[7] = argument;
        self.threads.push(ThreadSlot::new(id, state));
        Ok(id)
    }

    pub fn run(&mut self, trace: &mut dyn TraceSink) -> Result<u32, RunError> {
        if std::mem::replace(&mut self.started, true) {
            return Err(RunError::AlreadyRun);
        }
        let Self {
            memory,
            threads,
            services,
            mode,
            limit,
            ..
        } = self;
        let result = execute(memory, threads, services, *mode, *limit, trace);
        for slot in threads {
            if slot.status == ThreadStatus::Runnable {
                slot.status = ThreadStatus::Stopped;
            }
        }
        result
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
                    match linux
                        .thread_syscall(cpu, thread.id, memory)
                        .map_err(RunError::Syscall)?
                    {
                        ThreadSyscall::Continue => Action::Continue,
                        ThreadSyscall::Yield => Action::Yield,
                        ThreadSyscall::ExitThread(status) => Action::ExitThread(status.into()),
                        ThreadSyscall::ExitGroup(status) => Action::ExitGroup(status.into()),
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
                    ApiAction::Exit(status) => Action::ExitGroup(status),
                },
            ),
        }
    }
}

enum Action {
    Step,
    Continue,
    #[cfg(feature = "linux-elf")]
    Yield,
    #[cfg(feature = "linux-elf")]
    ExitThread(u32),
    ExitGroup(u32),
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
    threads: &mut [ThreadSlot],
    services: &mut Services<'_>,
    mode: ExecMode,
    mut remaining: u64,
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
    let mut ready = (0..threads.len()).collect::<VecDeque<_>>();
    while let Some(index) = ready.pop_front() {
        let slot = &mut threads[index];
        for _ in 0..QUANTUM {
            remaining = remaining.checked_sub(1).ok_or(RunError::Budget)?;
            match step_thread(&mut slot.thread, services, &mut engine)? {
                Action::Continue => {}
                #[cfg(feature = "linux-elf")]
                Action::Yield => break,
                #[cfg(feature = "linux-elf")]
                Action::ExitThread(status) => {
                    slot.status = ThreadStatus::Exited(status);
                    break;
                }
                Action::ExitGroup(status) => {
                    slot.status = ThreadStatus::Exited(status);
                    return Ok(status);
                }
                Action::Step => return Err(RunError::Backend),
            }
        }
        if slot.status == ThreadStatus::Runnable {
            ready.push_back(index);
        }
    }
    match threads[0].status {
        ThreadStatus::Exited(status) => Ok(status),
        _ => Err(RunError::Backend),
    }
}

fn step_thread(
    thread: &mut GuestThread,
    services: &mut Services<'_>,
    engine: &mut Engine<'_>,
) -> Result<Action, RunError> {
    let pc = thread.cpu.pc();
    let action = services.dispatch(thread, engine.memory());
    if !matches!(&action, Ok(Action::Step)) {
        engine.trace().record(TraceEvent::Step {
            thread: thread.id,
            pc,
        });
        return action;
    }
    match engine.step(thread)? {
        ExitReason::Dispatch(_) => Ok(Action::Continue),
        ExitReason::Fault(fault) => Err(RunError::Fault(fault)),
        _ => Err(RunError::Backend),
    }
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
