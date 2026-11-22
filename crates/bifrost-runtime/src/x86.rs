//! Convenience constructors for bare x86-64 dispatch. See [crate::raw] for both guest ISAs.

pub use crate::raw::{ExecutionBudget, ExecutionError, Executor, ExitReason};

#[cfg(feature = "interp")]
pub struct X86Interpreter<'a>(crate::raw::RawInterpreter<'a>);

#[cfg(feature = "interp")]
impl<'a> X86Interpreter<'a> {
    pub fn new(
        memory: &'a mut dyn bifrost_core::GuestMemory,
        trace: &'a mut dyn bifrost_core::TraceSink,
    ) -> Self {
        Self(crate::raw::RawInterpreter::for_isa(
            bifrost_core::GuestIsa::X86_64,
            memory,
            trace,
        ))
    }

    pub fn step(
        &mut self,
        thread: &mut bifrost_core::GuestThread,
    ) -> Result<ExitReason, ExecutionError> {
        self.0.step(thread)
    }
}

#[cfg(feature = "interp")]
impl Executor for X86Interpreter<'_> {
    type Error = ExecutionError;
    fn run(
        &mut self,
        thread: &mut bifrost_core::GuestThread,
        budget: ExecutionBudget,
    ) -> Result<ExitReason, ExecutionError> {
        self.0.run(thread, budget)
    }
}

#[cfg(all(
    feature = "jit-cranelift",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "windows")
))]
pub use crate::raw::SystemJitMemory;

#[cfg(feature = "jit-cranelift")]
pub use crate::raw::CacheStats;
#[cfg(feature = "jit-cranelift")]
pub use jit::X86Jit;

#[cfg(feature = "jit-cranelift")]
mod jit {
    use super::*;
    use crate::raw::RawJit;
    use bifrost_core::{GuestIsa, GuestThread, TraceSink};
    use bifrost_host::jit::JitMemoryManager;
    use bifrost_memory::AddressSpace;

    pub struct X86Jit<'a, M: JitMemoryManager>(RawJit<'a, M>);

    impl<'a, M: JitMemoryManager> X86Jit<'a, M> {
        pub fn new(
            memory: &'a mut AddressSpace,
            trace: &'a mut dyn TraceSink,
            manager: M,
        ) -> Result<Self, ExecutionError> {
            Ok(Self(RawJit::for_isa(
                GuestIsa::X86_64,
                memory,
                trace,
                manager,
            )?))
        }

        pub fn memory_mut(&mut self) -> &mut AddressSpace {
            self.0.memory_mut()
        }
        pub fn set_cache_capacity(&mut self, capacity: usize) {
            self.0.set_cache_capacity(capacity);
        }
        pub fn cache_stats(&self) -> CacheStats {
            self.0.cache_stats()
        }
        pub fn step(&mut self, thread: &mut GuestThread) -> Result<ExitReason, ExecutionError> {
            self.0.step(thread)
        }
    }

    impl<M: JitMemoryManager> Executor for X86Jit<'_, M> {
        type Error = ExecutionError;
        fn run(
            &mut self,
            thread: &mut GuestThread,
            budget: ExecutionBudget,
        ) -> Result<ExitReason, ExecutionError> {
            self.0.run(thread, budget)
        }
    }
}
