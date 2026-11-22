use super::{ExecutionError, check_isa, report_lift_failure};
use bifrost_codegen::{CodegenBackend, CodegenOptions};
use bifrost_codegen_clif::{BlockExit, Compiler};
use bifrost_core::{GuestThread, TraceEvent, TraceSink};
use bifrost_executor::{ExecutionBudget, Executor, ExitReason};
use bifrost_host::jit::JitMemoryManager;
use bifrost_ir::InstructionLifter;
use bifrost_lifter_x86::X86Lifter;
use bifrost_memory::AddressSpace;

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
pub use bifrost_host_linux::jit::LinuxJitMemory as SystemJitMemory;
#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
pub use bifrost_host_windows::jit::WindowsJitMemory as SystemJitMemory;

/// A Cranelift executor for one caller-owned guest address space.
pub struct X86Jit<'a, M: JitMemoryManager> {
    compiler: Compiler,
    memory: &'a mut AddressSpace,
    trace: &'a mut dyn TraceSink,
    manager: M,
}

impl<'a, M: JitMemoryManager> X86Jit<'a, M> {
    pub fn new(
        memory: &'a mut AddressSpace,
        trace: &'a mut dyn TraceSink,
        manager: M,
    ) -> Result<Self, ExecutionError> {
        Ok(Self {
            compiler: Compiler::new().map_err(ExecutionError::Compile)?,
            memory,
            trace,
            manager,
        })
    }

    pub fn memory_mut(&mut self) -> &mut AddressSpace {
        self.memory
    }

    pub fn step(&mut self, thread: &mut GuestThread) -> Result<ExitReason, ExecutionError> {
        check_isa(thread)?;
        if let Some(fault) = thread.pending_fault {
            return Ok(ExitReason::Fault(fault));
        }
        let pc = thread.cpu.pc();
        let function = match X86Lifter.lift_block(self.memory, pc, &Default::default()) {
            Ok(function) => function,
            Err(error) => return report_lift_failure(self.trace, thread, error),
        };
        let verified = function.verify().map_err(ExecutionError::Verify)?;
        let block = verified
            .blocks()
            .next()
            .ok_or(ExecutionError::UnexpectedIrExit)?;
        let code = self
            .compiler
            .compile(&block, &CodegenOptions::default())
            .map_err(ExecutionError::Compile)?;
        let published = code
            .publish(&self.manager)
            .map_err(ExecutionError::Publish)?;
        self.trace.record(TraceEvent::Step {
            thread: thread.id,
            pc,
        });
        match published
            .run(thread, self.memory)
            .map_err(ExecutionError::Jit)?
        {
            BlockExit::Dispatch(destination) => Ok(ExitReason::Dispatch(destination)),
            BlockExit::Fault(fault) => {
                self.trace.record(TraceEvent::Fault(fault));
                Ok(ExitReason::Fault(fault))
            }
        }
    }
}

impl<M: JitMemoryManager> Executor for X86Jit<'_, M> {
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
