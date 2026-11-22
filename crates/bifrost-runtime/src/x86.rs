//! Bare x86-64 dispatch over caller-owned guest memory.

#[cfg(feature = "interp")]
use bifrost_core::GuestMemory;
use bifrost_core::{
    AccessKind, FaultClass, GuestFault, GuestIsa, GuestThread, TraceEvent, TraceSink,
};
use bifrost_decoder_x86::DecodeError;
#[cfg(feature = "interp")]
use bifrost_interpreter::{Interpreter, InterpreterError, StepOutcome};
#[cfg(feature = "interp")]
use bifrost_ir::InstructionLifter;
use bifrost_ir::VerifyError;
use bifrost_lifter_x86::{LiftError, X86Lifter};
use std::fmt;

pub use bifrost_executor::{ExecutionBudget, Executor, ExitReason};

#[cfg(feature = "jit-cranelift")]
mod jit;
#[cfg(all(
    feature = "jit-cranelift",
    target_arch = "x86_64",
    any(target_os = "linux", target_os = "windows")
))]
pub use jit::SystemJitMemory;
#[cfg(feature = "jit-cranelift")]
pub use jit::X86Jit;

#[derive(Debug)]
#[non_exhaustive]
pub enum ExecutionError {
    IsaMismatch(GuestIsa),
    Lift(LiftError),
    Verify(VerifyError),
    #[cfg(feature = "interp")]
    Interpreter(InterpreterError),
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
                "x86-64 execution requires x86-64 guest state, received {actual:?}"
            ),
            Self::Lift(error) => error.fmt(f),
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
            Self::Lift(error) => Some(error),
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

/// Executes the documented scalar subset. Code is fetched fresh for every instruction.
/// All branch and stack addresses belong to guest memory.
#[cfg(feature = "interp")]
pub struct X86Interpreter<'a> {
    memory: &'a mut dyn GuestMemory,
    trace: &'a mut dyn TraceSink,
}

#[cfg(feature = "interp")]
impl<'a> X86Interpreter<'a> {
    /// Pass `&mut ()` to discard trace events.
    pub fn new(memory: &'a mut dyn GuestMemory, trace: &'a mut dyn TraceSink) -> Self {
        Self { memory, trace }
    }

    pub fn step(&mut self, thread: &mut GuestThread) -> Result<ExitReason, ExecutionError> {
        check_isa(thread)?;
        if let Some(fault) = thread.pending_fault {
            return Ok(ExitReason::Fault(fault));
        }

        let pc = thread.cpu.pc();
        let lifted = match X86Lifter.lift_block(self.memory, pc, &Default::default()) {
            Ok(function) => function,
            Err(error) => return report_lift_failure(self.trace, thread, error),
        };
        let verified = lifted.verify().map_err(ExecutionError::Verify)?;
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

fn check_isa(thread: &GuestThread) -> Result<(), ExecutionError> {
    let actual = thread.cpu.isa();
    if actual != GuestIsa::X86_64 {
        return Err(ExecutionError::IsaMismatch(actual));
    }
    Ok(())
}

fn report_lift_failure(
    trace: &mut dyn TraceSink,
    thread: &mut GuestThread,
    error: LiftError,
) -> Result<ExitReason, ExecutionError> {
    let pc = thread.cpu.pc();
    trace.record(TraceEvent::Step {
        thread: thread.id,
        pc,
    });
    let fault = match error {
        LiftError::Decode(DecodeError::Fetch { source, .. }) => {
            source.guest_fault(pc, thread.id, AccessKind::Execute)
        }
        LiftError::Decode(DecodeError::Invalid { .. }) => Some(GuestFault {
            pc,
            thread: thread.id,
            class: FaultClass::IllegalInstruction,
            memory_access: None,
        }),
        LiftError::Decode(DecodeError::NonCanonical { address, .. }) => Some(GuestFault {
            pc,
            thread: thread.id,
            class: FaultClass::NonCanonicalAddress,
            memory_access: Some((address, AccessKind::Execute)),
        }),
        _ => None,
    };
    let Some(fault) = fault else {
        return Err(ExecutionError::Lift(error));
    };
    thread.pending_fault = Some(fault);
    trace.record(TraceEvent::Fault(fault));
    Ok(ExitReason::Fault(fault))
}

#[cfg(feature = "interp")]
impl Executor for X86Interpreter<'_> {
    type Error = ExecutionError;

    fn run(
        &mut self,
        thread: &mut GuestThread,
        budget: ExecutionBudget,
    ) -> Result<ExitReason, Self::Error> {
        check_isa(thread)?;
        if let Some(fault) = thread.pending_fault {
            return Ok(ExitReason::Fault(fault));
        }
        for _ in 0..budget.instructions {
            match self.step(thread)? {
                ExitReason::Dispatch(_) => {}
                reason => return Ok(reason),
            }
        }
        Ok(ExitReason::BudgetExhausted)
    }
}
