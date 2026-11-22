use super::{ExecutionError, check_isa, report_lift_failure};
use bifrost_codegen::{CodegenBackend, CodegenOptions};
use bifrost_codegen_clif::{BlockExit, Compiler, MachineCode, PublishedBlock};
use bifrost_core::{GuestAddress, GuestThread, TraceEvent, TraceSink};
use bifrost_decoder_x86::MAX_INSTRUCTION_BYTES;
use bifrost_executor::{ExecutionBudget, Executor, ExitReason};
use bifrost_host::jit::JitMemoryManager;
use bifrost_ir::{CodeSpan, InstructionLifter};
use bifrost_lifter_x86::X86Lifter;
use bifrost_memory::{AddressSpace, AddressSpaceId};
use std::collections::BTreeMap;

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
pub use bifrost_host_linux::jit::LinuxJitMemory as SystemJitMemory;
#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
pub use bifrost_host_windows::jit::WindowsJitMemory as SystemJitMemory;

const DEFAULT_CACHE_CAPACITY: usize = 256;

struct CachedBlock<Image> {
    span: CodeSpan,
    generations: [u64; MAX_INSTRUCTION_BYTES],
    code: PublishedBlock<Image>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub resident: usize,
}

fn code_generations(memory: &AddressSpace, span: CodeSpan) -> Option<[u64; MAX_INSTRUCTION_BYTES]> {
    let page_size = memory.page_size() as u64;
    let first = span.start.0 / page_size;
    let last = span
        .start
        .checked_add(u64::from(span.len.checked_sub(1)?))?
        .0
        / page_size;
    let mut generations = [0; MAX_INSTRUCTION_BYTES];
    for (i, page) in (first..=last).enumerate() {
        *generations.get_mut(i)? = memory.generation_at(GuestAddress(page * page_size))?;
    }
    Some(generations)
}

/// A Cranelift executor for one caller-owned guest address space.
pub struct X86Jit<'a, M: JitMemoryManager> {
    compiler: Compiler,
    memory: &'a mut AddressSpace,
    trace: &'a mut dyn TraceSink,
    manager: M,
    cache: BTreeMap<GuestAddress, CachedBlock<M::Image>>,
    cache_owner: AddressSpaceId,
    cache_capacity: usize,
    hits: u64,
    misses: u64,
}

impl<'a, M: JitMemoryManager> X86Jit<'a, M> {
    pub fn new(
        memory: &'a mut AddressSpace,
        trace: &'a mut dyn TraceSink,
        manager: M,
    ) -> Result<Self, ExecutionError> {
        let cache_owner = memory.id();
        Ok(Self {
            compiler: Compiler::new().map_err(ExecutionError::Compile)?,
            memory,
            trace,
            manager,
            cache: BTreeMap::new(),
            cache_owner,
            cache_capacity: DEFAULT_CACHE_CAPACITY,
            hits: 0,
            misses: 0,
        })
    }

    pub fn memory_mut(&mut self) -> &mut AddressSpace {
        self.memory
    }

    /// Bounds the number of resident executable blocks. Zero disables caching.
    pub fn set_cache_capacity(&mut self, capacity: usize) {
        self.cache_capacity = capacity;
        while self.cache.len() > capacity {
            self.cache.pop_first();
        }
    }

    pub fn cache_stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            resident: self.cache.len(),
        }
    }

    pub fn step(&mut self, thread: &mut GuestThread) -> Result<ExitReason, ExecutionError> {
        check_isa(thread)?;
        if let Some(fault) = thread.pending_fault {
            return Ok(ExitReason::Fault(fault));
        }
        let pc = thread.cpu.pc();
        if self.cache_owner != self.memory.id() {
            self.cache.clear();
            self.cache_owner = self.memory.id();
        }
        if self.cache.get(&pc).is_some_and(|entry| {
            code_generations(self.memory, entry.span) != Some(entry.generations)
        }) {
            self.cache.remove(&pc);
        }
        let mut uncached = None;
        if !self.cache.contains_key(&pc) {
            self.misses = self.misses.saturating_add(1);
            let code = match self.compile(pc) {
                Ok(code) => code,
                Err(ExecutionError::Lift(error)) => {
                    return report_lift_failure(self.trace, thread, error);
                }
                Err(error) => return Err(error),
            };
            let span = code.guest_span();
            let published = code
                .publish(&self.manager)
                .map_err(ExecutionError::Publish)?;
            if self.cache_capacity != 0
                && let Some(generations) = code_generations(self.memory, span)
            {
                if self.cache.len() == self.cache_capacity {
                    self.cache.pop_first();
                }
                self.cache.insert(
                    pc,
                    CachedBlock {
                        span,
                        generations,
                        code: published,
                    },
                );
            } else {
                uncached = Some(published);
            }
        } else {
            self.hits = self.hits.saturating_add(1);
        }
        let published = match uncached.as_ref() {
            Some(code) => code,
            None => &self.cache.get(&pc).expect("cache hit or insertion").code,
        };
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

    fn compile(&mut self, pc: GuestAddress) -> Result<MachineCode, ExecutionError> {
        let function = X86Lifter
            .lift_block(self.memory, pc, &Default::default())
            .map_err(ExecutionError::Lift)?;
        let verified = function.verify().map_err(ExecutionError::Verify)?;
        let block = verified
            .blocks()
            .next()
            .ok_or(ExecutionError::UnexpectedIrExit)?;
        self.compiler
            .compile(&block, &CodegenOptions::default())
            .map_err(ExecutionError::Compile)
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
