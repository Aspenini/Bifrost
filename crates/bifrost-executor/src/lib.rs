//! Execution and scheduling contracts. Backends own their dispatch state.

use bifrost_core::{GuestAddress, GuestFault, GuestThread};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionBudget {
    pub instructions: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ExitReason {
    BudgetExhausted,
    Dispatch(GuestAddress),
    Fault(GuestFault),
    /// Guest state is unchanged. The host handles the call and advances the guest PC.
    Arm64SupervisorCall {
        pc: GuestAddress,
        immediate: u16,
    },
}

/// The caller hands over one thread for the duration of a bounded run.
/// The implementation must make guest state inspectable at every exit.
pub trait Executor {
    type Error: std::error::Error + Send + Sync + 'static;

    fn run(
        &mut self,
        thread: &mut GuestThread,
        budget: ExecutionBudget,
    ) -> Result<ExitReason, Self::Error>;
}

pub trait GuestThreadScheduler {
    fn next_thread(&mut self) -> Option<&mut GuestThread>;
}
