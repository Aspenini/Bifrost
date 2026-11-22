use crate::{BridgeError, BridgeRegistry};
use bifrost_abi::{AbiDescriptor, AbiError, Location, ReturnAddress, Scalar};
use bifrost_core::{CpuState, GuestAddress, GuestIsa, GuestMemory, GuestThread};
use bifrost_executor::{ExecutionBudget, Executor, ExitReason};

const FRAME_BYTES: u64 = 4096;
const MAX_CALLBACK_DEPTH: u8 = 8;

/// A bounded backend with exclusive access to the same guest memory used by the bridge.
pub trait BridgeBackend {
    type Error: std::error::Error + Send + Sync + 'static;
    fn step(&mut self, thread: &mut GuestThread) -> Result<ExitReason, Self::Error>;
    fn memory(&mut self) -> &mut dyn GuestMemory;
}

/// A caller-reserved RW guest stack for synchronous callbacks, with 4 KiB per nesting level.
/// The return marker is a dedicated, non-executable guest address.
#[derive(Clone, Copy, Debug)]
pub struct CallbackStack {
    start: GuestAddress,
    depth: u8,
    marker: GuestAddress,
}

impl CallbackStack {
    pub fn new(start: GuestAddress, depth: u8, marker: GuestAddress) -> Result<Self, BridgeError> {
        let end = start
            .checked_add(FRAME_BYTES * u64::from(depth))
            .ok_or(BridgeError::InvalidCallbackStack)?;
        if depth == 0
            || depth > MAX_CALLBACK_DEPTH
            || !start.0.is_multiple_of(FRAME_BYTES)
            || (start..end).contains(&marker)
        {
            return Err(BridgeError::InvalidCallbackStack);
        }
        Ok(Self {
            start,
            depth,
            marker,
        })
    }
}

/// Dispatches registered native entry PCs between guest instructions.
pub struct BridgeExecutor<'a, B> {
    abi: &'a AbiDescriptor,
    registry: &'a BridgeRegistry,
    callback: Option<CallbackStack>,
    backend: B,
}

impl<'a, B: BridgeBackend> BridgeExecutor<'a, B> {
    pub fn new(abi: &'a AbiDescriptor, registry: &'a BridgeRegistry, backend: B) -> Self {
        Self {
            abi,
            registry,
            callback: None,
            backend,
        }
    }

    pub fn with_callback_stack(mut self, stack: CallbackStack) -> Result<Self, BridgeError> {
        self.abi.validate_code_address(stack.marker)?;
        self.abi
            .validate_span(stack.start, (FRAME_BYTES * u64::from(stack.depth)) as usize)?;
        if self.registry.contains(stack.marker) {
            return Err(BridgeError::InvalidCallbackStack);
        }
        self.callback = Some(stack);
        Ok(self)
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    fn step(
        &mut self,
        thread: &mut GuestThread,
        remaining: &mut u64,
        depth: u8,
    ) -> Result<ExitReason, BridgeError> {
        if *remaining == 0 {
            return Ok(ExitReason::BudgetExhausted);
        }
        *remaining -= 1;
        if !self.registry.contains(thread.cpu.pc()) {
            return self
                .backend
                .step(thread)
                .map_err(|error| BridgeError::Backend(Box::new(error)));
        }

        let mut call = self
            .registry
            .prepare(self.abi, &thread.cpu, self.backend.memory())?;
        let result = if call.callback_address().is_some() {
            let stack = self.callback.ok_or(BridgeError::CallbackUnavailable)?;
            if depth >= stack.depth {
                return Err(BridgeError::CallbackDepth);
            }
            if *remaining == 0 {
                return Err(BridgeError::CallbackBudget);
            }
            call.call_native(Some(&mut |entry, value| {
                self.callback(thread, remaining, depth, entry, value)
            }))?
        } else {
            call.call_native(None)?
        };
        call.finish(&mut thread.cpu, self.backend.memory(), result)?;
        Ok(ExitReason::Dispatch(thread.cpu.pc()))
    }

    fn callback(
        &mut self,
        outer: &GuestThread,
        remaining: &mut u64,
        depth: u8,
        entry: GuestAddress,
        value: u64,
    ) -> Result<u64, BridgeError> {
        let stack = self.callback.ok_or(BridgeError::CallbackUnavailable)?;
        if depth >= stack.depth {
            return Err(BridgeError::CallbackDepth);
        }
        if *remaining == 0 {
            return Err(BridgeError::CallbackBudget);
        }
        let mut guest = outer.clone();
        guest.pending_fault = None;
        guest.cpu.set_pc(entry);
        let base = stack
            .start
            .checked_add(FRAME_BYTES * u64::from(stack.depth - depth - 1))
            .ok_or(BridgeError::InvalidCallbackStack)?;
        let top = base
            .checked_add(FRAME_BYTES)
            .ok_or(BridgeError::InvalidCallbackStack)?;
        let memory = self.backend.memory();
        let mut marker = [0; 4];
        let length = if self.abi.isa == GuestIsa::AArch64 {
            4
        } else {
            1
        };
        if memory.fetch(stack.marker, &mut marker[..length]).is_ok() {
            return Err(BridgeError::InvalidCallbackStack);
        }
        memory.check_write(base, FRAME_BYTES as usize)?;
        let ret = match &mut guest.cpu {
            CpuState::X86_64(state) => {
                state.gpr[4] = top.0
                    - u64::from(self.abi.shadow_bytes)
                    - u64::from(self.abi.entry_stack_bias());
                Location::Stack(0)
            }
            CpuState::AArch64(state) => {
                state.sp = top;
                let ReturnAddress::LinkRegister(reg) = self.abi.return_address else {
                    return Err(BridgeError::InvalidCallbackStack);
                };
                Location::Integer(reg)
            }
            _ => return Err(BridgeError::InvalidCallbackStack),
        };
        self.abi
            .write_word(&mut guest.cpu, memory, ret, stack.marker.0)?;
        let argument = Location::Integer(self.abi.integer_args[0]);
        self.abi
            .write_word(&mut guest.cpu, memory, argument, value)?;
        let expected_stack = GuestAddress(top.0 - u64::from(self.abi.shadow_bytes));

        loop {
            if guest.cpu.pc() == stack.marker {
                if guest.cpu.sp() != expected_stack {
                    return Err(BridgeError::InvalidCallbackReturn);
                }
                let result = self.abi.scalar_return(Scalar::U64);
                return self
                    .abi
                    .read_result_register(&guest.cpu, result)
                    .map_err(Into::into);
            }
            match self.step(&mut guest, remaining, depth + 1)? {
                ExitReason::Dispatch(_) => {}
                ExitReason::Fault(fault) => return Err(BridgeError::CallbackFault(fault)),
                ExitReason::BudgetExhausted => return Err(BridgeError::CallbackBudget),
                _ => return Err(BridgeError::InvalidBackendExit),
            }
        }
    }
}

impl<B: BridgeBackend> Executor for BridgeExecutor<'_, B> {
    type Error = BridgeError;

    fn run(
        &mut self,
        thread: &mut GuestThread,
        budget: ExecutionBudget,
    ) -> Result<ExitReason, BridgeError> {
        if thread.cpu.isa() != self.abi.isa {
            return Err(AbiError::IsaMismatch {
                expected: self.abi.isa,
                actual: thread.cpu.isa(),
            }
            .into());
        }
        if let Some(fault) = thread.pending_fault {
            return Ok(ExitReason::Fault(fault));
        }
        let mut remaining = budget.instructions;
        loop {
            match self.step(thread, &mut remaining, 0)? {
                ExitReason::Dispatch(_) => {}
                reason => return Ok(reason),
            }
        }
    }
}
