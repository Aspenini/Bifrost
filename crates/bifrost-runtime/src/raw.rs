//! Common embedding API for the bare x86-64 and ARM64 execution profiles.

#[cfg(feature = "interp")]
use bifrost_core::GuestMemory;
use bifrost_core::{
    AccessKind, FaultClass, GuestAddress, GuestFault, GuestIsa, GuestMemoryRead, GuestTarget,
    GuestThread, TraceEvent, TraceSink,
};
use bifrost_ir::{InstructionLifter, IrFunction, VerifyError};
use std::fmt;

#[cfg(feature = "guest-aarch64")]
mod arm64_crc;
#[cfg(feature = "guest-aarch64")]
mod arm64_float;
#[cfg(feature = "jit-cranelift")]
mod jit;
#[cfg(feature = "jit-cranelift")]
pub use jit::{CacheStats, RawJit};

#[cfg(all(
    feature = "jit-cranelift",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "windows")
))]
pub use jit::SystemJitMemory;

pub use bifrost_executor::{ExecutionBudget, Executor, ExitReason};

#[derive(Debug)]
#[non_exhaustive]
pub enum ExecutionError {
    IsaMismatch(GuestIsa),
    UnsupportedTarget(GuestTarget),
    #[cfg(feature = "guest-x86_64")]
    Lift(bifrost_lifter_x86::LiftError),
    #[cfg(feature = "guest-aarch64")]
    Arm64Lift(bifrost_lifter_arm64::LiftError),
    #[cfg(feature = "guest-aarch64")]
    UnsupportedArm64Fpcr {
        pc: GuestAddress,
        fpcr: u32,
    },
    Verify(VerifyError),
    #[cfg(feature = "interp")]
    Interpreter(bifrost_interpreter::InterpreterError),
    #[cfg(feature = "jit-cranelift")]
    Compile(bifrost_codegen_clif::CompileError),
    #[cfg(feature = "jit-cranelift")]
    Publish(std::io::Error),
    #[cfg(feature = "jit-cranelift")]
    Jit(bifrost_codegen_clif::ExecutionError),
    UnexpectedIrExit,
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IsaMismatch(actual) => write!(
                f,
                "guest state ISA {actual:?} does not match the executor target"
            ),
            Self::UnsupportedTarget(target) => {
                write!(f, "unsupported raw guest profile: {target:?}")
            }
            #[cfg(feature = "guest-x86_64")]
            Self::Lift(error) => error.fmt(f),
            #[cfg(feature = "guest-aarch64")]
            Self::Arm64Lift(error) => error.fmt(f),
            #[cfg(feature = "guest-aarch64")]
            Self::UnsupportedArm64Fpcr { pc, fpcr } => {
                write!(
                    f,
                    "unsupported ARM64 floating-point control {fpcr:#010x} at {:#x}",
                    pc.0
                )
            }
            Self::Verify(error) => error.fmt(f),
            #[cfg(feature = "interp")]
            Self::Interpreter(error) => error.fmt(f),
            #[cfg(feature = "jit-cranelift")]
            Self::Compile(error) => error.fmt(f),
            #[cfg(feature = "jit-cranelift")]
            Self::Publish(error) => write!(f, "could not publish executable JIT code: {error}"),
            #[cfg(feature = "jit-cranelift")]
            Self::Jit(error) => error.fmt(f),
            Self::UnexpectedIrExit => {
                f.write_str("single-instruction translation did not dispatch or fault")
            }
        }
    }
}

impl std::error::Error for ExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(feature = "guest-x86_64")]
            Self::Lift(error) => Some(error),
            #[cfg(feature = "guest-aarch64")]
            Self::Arm64Lift(error) => Some(error),
            Self::Verify(error) => Some(error),
            #[cfg(feature = "interp")]
            Self::Interpreter(error) => Some(error),
            #[cfg(feature = "jit-cranelift")]
            Self::Compile(error) => Some(error),
            #[cfg(feature = "jit-cranelift")]
            Self::Publish(error) => Some(error),
            #[cfg(feature = "jit-cranelift")]
            Self::Jit(error) => Some(error),
            _ => None,
        }
    }
}

fn validate(target: &GuestTarget) -> Result<GuestIsa, ExecutionError> {
    if crate::supports_raw_profile(target) {
        Ok(target.isa())
    } else {
        Err(ExecutionError::UnsupportedTarget(target.clone()))
    }
}

fn check_isa(expected: GuestIsa, thread: &GuestThread) -> Result<(), ExecutionError> {
    let actual = thread.cpu.isa();
    if actual == expected {
        Ok(())
    } else {
        Err(ExecutionError::IsaMismatch(actual))
    }
}

fn lift(
    isa: GuestIsa,
    memory: &dyn GuestMemoryRead,
    pc: GuestAddress,
) -> Result<IrFunction, ExecutionError> {
    match isa {
        #[cfg(feature = "guest-x86_64")]
        GuestIsa::X86_64 => bifrost_lifter_x86::X86Lifter
            .lift_block(memory, pc, &Default::default())
            .map_err(ExecutionError::Lift),
        #[cfg(feature = "guest-aarch64")]
        GuestIsa::AArch64 => bifrost_lifter_arm64::Arm64Lifter
            .lift_block(memory, pc, &Default::default())
            .map_err(ExecutionError::Arm64Lift),
        _ => unreachable!("executor targets are validated at construction"),
    }
}

fn report_lift_failure(
    trace: &mut dyn TraceSink,
    thread: &mut GuestThread,
    error: ExecutionError,
) -> Result<ExitReason, ExecutionError> {
    let pc = thread.cpu.pc();
    trace.record(TraceEvent::Step {
        thread: thread.id,
        pc,
    });
    let fault = |class, memory_access| {
        Some(GuestFault {
            pc,
            thread: thread.id,
            class,
            memory_access,
        })
    };
    let failure = match error {
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::SupervisorCall {
            pc,
            immediate,
        }) => return Ok(ExitReason::Arm64SupervisorCall { pc, immediate }),
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::ReadCounter {
            pc,
            register,
            rt,
        }) => return Ok(ExitReason::Arm64ReadCounter { pc, register, rt }),
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::Crc32 { pc, operation }) => {
            let bifrost_core::CpuState::AArch64(cpu) = &mut thread.cpu else {
                unreachable!("ARM64 instruction requires ARM64 state");
            };
            arm64_crc::execute(cpu, pc, operation);
            return Ok(ExitReason::Dispatch(cpu.pc));
        }
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::FloatMultiplyAdd {
            pc,
            operation,
        }) => {
            let bifrost_core::CpuState::AArch64(cpu) = &mut thread.cpu else {
                unreachable!("ARM64 instruction requires ARM64 state");
            };
            arm64_float::multiply_add(cpu, pc, operation)?;
            return Ok(ExitReason::Dispatch(cpu.pc));
        }
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::IntegerToFloat {
            pc,
            operation,
        }) => {
            let bifrost_core::CpuState::AArch64(cpu) = &mut thread.cpu else {
                unreachable!("ARM64 instruction requires ARM64 state");
            };
            arm64_float::integer_to_float(cpu, pc, operation)?;
            return Ok(ExitReason::Dispatch(cpu.pc));
        }
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::FloatBinary {
            pc,
            operation,
        }) => {
            let bifrost_core::CpuState::AArch64(cpu) = &mut thread.cpu else {
                unreachable!("ARM64 instruction requires ARM64 state");
            };
            arm64_float::binary(cpu, pc, operation)?;
            return Ok(ExitReason::Dispatch(cpu.pc));
        }
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::FloatCompare {
            pc,
            operation,
        }) => {
            let bifrost_core::CpuState::AArch64(cpu) = &mut thread.cpu else {
                unreachable!("ARM64 instruction requires ARM64 state");
            };
            arm64_float::compare(cpu, pc, operation)?;
            return Ok(ExitReason::Dispatch(cpu.pc));
        }
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::FloatSign { pc, operation }) => {
            let bifrost_core::CpuState::AArch64(cpu) = &mut thread.cpu else {
                unreachable!("ARM64 instruction requires ARM64 state");
            };
            arm64_float::sign(cpu, pc, operation)?;
            return Ok(ExitReason::Dispatch(cpu.pc));
        }
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::FloatConvert {
            pc,
            operation,
        }) => {
            let bifrost_core::CpuState::AArch64(cpu) = &mut thread.cpu else {
                unreachable!("ARM64 instruction requires ARM64 state");
            };
            arm64_float::convert(cpu, pc, operation)?;
            return Ok(ExitReason::Dispatch(cpu.pc));
        }
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::PointerAuth {
            pc,
            operation,
        }) => {
            return Ok(ExitReason::Arm64PointerAuth { pc, operation });
        }
        #[cfg(feature = "guest-x86_64")]
        ExecutionError::Lift(bifrost_lifter_x86::LiftError::Decode(error)) => {
            use bifrost_decoder_x86::DecodeError;
            match error {
                DecodeError::Fetch { source, .. } => {
                    source.guest_fault(pc, thread.id, AccessKind::Execute)
                }
                DecodeError::Invalid { .. } => fault(FaultClass::IllegalInstruction, None),
                DecodeError::NonCanonical { address, .. } => fault(
                    FaultClass::NonCanonicalAddress,
                    Some((address, AccessKind::Execute)),
                ),
                _ => None,
            }
        }
        #[cfg(feature = "guest-aarch64")]
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::Decode(error)) => {
            use bifrost_decoder_arm64::DecodeError;
            match error {
                DecodeError::Fetch { source, .. } => {
                    source.guest_fault(pc, thread.id, AccessKind::Execute)
                }
                DecodeError::Alignment(address) => {
                    fault(FaultClass::Alignment, Some((address, AccessKind::Execute)))
                }
                _ => None,
            }
        }
        _ => None,
    };
    let Some(fault) = failure else {
        return Err(error);
    };
    thread.pending_fault = Some(fault);
    trace.record(TraceEvent::Fault(fault));
    Ok(ExitReason::Fault(fault))
}

fn run_steps(
    isa: GuestIsa,
    thread: &mut GuestThread,
    budget: ExecutionBudget,
    mut step: impl FnMut(&mut GuestThread) -> Result<ExitReason, ExecutionError>,
) -> Result<ExitReason, ExecutionError> {
    check_isa(isa, thread)?;
    if let Some(fault) = thread.pending_fault {
        return Ok(ExitReason::Fault(fault));
    }
    for _ in 0..budget.instructions {
        match step(thread)? {
            ExitReason::Dispatch(_) => {}
            reason => return Ok(reason),
        }
    }
    Ok(ExitReason::BudgetExhausted)
}

/// Interprets decoded and verified raw guest instructions using the selected baseline guest.
#[cfg(feature = "interp")]
pub struct RawInterpreter<'a> {
    isa: GuestIsa,
    memory: &'a mut dyn GuestMemory,
    trace: &'a mut dyn TraceSink,
}

#[cfg(feature = "interp")]
impl<'a> RawInterpreter<'a> {
    #[cfg(any(
        feature = "linux-elf",
        feature = "windows-pe",
        feature = "macos-standalone"
    ))]
    pub(crate) fn trace_mut(&mut self) -> &mut dyn TraceSink {
        self.trace
    }

    pub fn memory_mut(&mut self) -> &mut dyn GuestMemory {
        self.memory
    }

    pub fn new(
        target: &GuestTarget,
        memory: &'a mut dyn GuestMemory,
        trace: &'a mut dyn TraceSink,
    ) -> Result<Self, ExecutionError> {
        Ok(Self::for_isa(validate(target)?, memory, trace))
    }

    pub(crate) fn for_isa(
        isa: GuestIsa,
        memory: &'a mut dyn GuestMemory,
        trace: &'a mut dyn TraceSink,
    ) -> Self {
        Self { isa, memory, trace }
    }

    pub fn step(&mut self, thread: &mut GuestThread) -> Result<ExitReason, ExecutionError> {
        use bifrost_interpreter::{Interpreter, StepOutcome};

        check_isa(self.isa, thread)?;
        if let Some(fault) = thread.pending_fault {
            return Ok(ExitReason::Fault(fault));
        }
        let function = match lift(self.isa, self.memory, thread.cpu.pc()) {
            Ok(function) => function,
            Err(error) => return report_lift_failure(self.trace, thread, error),
        };
        let verified = function.verify().map_err(ExecutionError::Verify)?;
        let mut interpreter =
            Interpreter::new(&verified, &[]).map_err(ExecutionError::Interpreter)?;
        match interpreter
            .step(thread, self.memory, self.trace)
            .map_err(ExecutionError::Interpreter)?
        {
            StepOutcome::Dispatch(next) => Ok(ExitReason::Dispatch(next)),
            StepOutcome::Fault(fault) => Ok(ExitReason::Fault(fault)),
            _ => Err(ExecutionError::UnexpectedIrExit),
        }
    }
}

#[cfg(feature = "interp")]
impl Executor for RawInterpreter<'_> {
    type Error = ExecutionError;

    fn run(
        &mut self,
        thread: &mut GuestThread,
        budget: ExecutionBudget,
    ) -> Result<ExitReason, Self::Error> {
        run_steps(self.isa, thread, budget, |thread| self.step(thread))
    }
}

#[cfg(all(feature = "interp", feature = "abi-bridge"))]
impl bifrost_bridge::BridgeBackend for RawInterpreter<'_> {
    type Error = ExecutionError;
    fn step(&mut self, thread: &mut GuestThread) -> Result<ExitReason, ExecutionError> {
        RawInterpreter::step(self, thread)
    }
    fn memory(&mut self) -> &mut dyn bifrost_core::GuestMemory {
        self.memory
    }
}
