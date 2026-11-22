//! Bounded, resumable evaluation of verified Bifröst IR.

mod evaluate;
mod value;

use bifrost_core::{
    FaultClass, GuestAddress, GuestFault, GuestIsa, GuestMemory, GuestThread, MemoryError,
    TraceEvent, TraceSink,
};
use bifrost_executor::{ExecutionBudget, ExitReason};
use bifrost_ir::{BlockId, Edge, Exit, IrBlock, Type, ValueId, VerifiedIrFunction};
use evaluate::EvalError;
use std::{collections::BTreeMap, fmt};
use value::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StepOutcome {
    Continue(GuestAddress),
    Dispatch(GuestAddress),
    Fault(GuestFault),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InterpreterError {
    ArgumentCount {
        expected: usize,
        actual: usize,
    },
    ArgumentWidth {
        index: usize,
        ty: Type,
    },
    IsaMismatch {
        expected: GuestIsa,
        actual: GuestIsa,
    },
    PcMismatch {
        expected: GuestAddress,
        actual: GuestAddress,
    },
    Finished,
    UnsupportedIr,
    Memory(MemoryError),
}

impl fmt::Display for InterpreterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArgumentCount { expected, actual } => write!(
                f,
                "expected {expected} IR entry arguments, received {actual}"
            ),
            Self::ArgumentWidth { index, ty } => {
                write!(f, "IR entry argument {index} exceeds the width of {ty:?}")
            }
            Self::IsaMismatch { expected, actual } => {
                write!(f, "IR targets {expected:?} but thread uses {actual:?}")
            }
            Self::PcMismatch { expected, actual } => write!(
                f,
                "expected guest PC {:#x}, received {:#x}",
                expected.0, actual.0
            ),
            Self::Finished => f.write_str("IR execution has already exited"),
            Self::UnsupportedIr => f.write_str("IR operation is unsupported by this interpreter"),
            Self::Memory(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for InterpreterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Memory(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Local {
    Data(Value),
    Guard(GuestAddress),
}

type Locals = BTreeMap<ValueId, Local>;

/// Resumable evaluation of one verified IR function. Each step executes a group of operations
/// at one guest PC within a block, including the terminator when it has that PC.
pub struct Interpreter<'a> {
    function: &'a VerifiedIrFunction,
    blocks: BTreeMap<BlockId, usize>,
    block: usize,
    offset: usize,
    locals: Locals,
    finished: bool,
}

impl<'a> Interpreter<'a> {
    /// Entry arguments carry the raw bits of the verified entry block's parameters.
    pub fn new(
        function: &'a VerifiedIrFunction,
        arguments: &[u128],
    ) -> Result<Self, InterpreterError> {
        let ir = function.as_ref();
        let blocks: BTreeMap<_, _> = ir
            .blocks
            .iter()
            .enumerate()
            .map(|(i, block)| (block.id, i))
            .collect();
        let block = blocks[&ir.entry];
        let params = &ir.blocks[block].parameters;
        if params.len() != arguments.len() {
            return Err(InterpreterError::ArgumentCount {
                expected: params.len(),
                actual: arguments.len(),
            });
        }
        let mut locals = Locals::new();
        for (index, (param, &bits)) in params.iter().zip(arguments).enumerate() {
            let value = Value::new(param.ty, bits);
            if value.bits != bits {
                return Err(InterpreterError::ArgumentWidth {
                    index,
                    ty: param.ty,
                });
            }
            locals.insert(param.value, Local::Data(value));
        }
        Ok(Self {
            function,
            blocks,
            block,
            offset: 0,
            locals,
            finished: false,
        })
    }

    /// The thread PC must agree with the next operation (or terminator) in this frame.
    pub fn step(
        &mut self,
        thread: &mut GuestThread,
        memory: &mut dyn GuestMemory,
        trace: &mut dyn TraceSink,
    ) -> Result<StepOutcome, InterpreterError> {
        let expected = self.function.as_ref().target.isa();
        let actual = thread.cpu.isa();
        if expected != actual {
            return Err(InterpreterError::IsaMismatch { expected, actual });
        }
        if let Some(fault) = thread.pending_fault {
            return Ok(StepOutcome::Fault(fault));
        }
        if self.finished {
            return Err(InterpreterError::Finished);
        }

        let block = &self.function.as_ref().blocks[self.block];
        let pc = next_pc(block, self.offset);
        let actual = thread.cpu.pc();
        if pc != actual {
            return Err(InterpreterError::PcMismatch {
                expected: pc,
                actual,
            });
        }
        trace.record(TraceEvent::Step {
            thread: thread.id,
            pc,
        });

        while let Some(inst) = block
            .instructions
            .get(self.offset)
            .filter(|inst| inst.pc == pc)
        {
            match evaluate::instruction(inst.op, pc, thread, memory, &mut self.locals) {
                Ok(()) => self.offset += 1,
                Err(EvalError::Guest(fault)) => return Ok(self.fault(thread, trace, fault)),
                Err(EvalError::Backend(error)) => {
                    self.finished = true;
                    return Err(error);
                }
            }
        }

        if self.offset == block.instructions.len() && block.exit_pc == pc {
            return self.terminate(block, thread, trace);
        }

        let next = next_pc(block, self.offset);
        thread.cpu.set_pc(next);
        Ok(StepOutcome::Continue(next))
    }

    pub fn run(
        &mut self,
        thread: &mut GuestThread,
        memory: &mut dyn GuestMemory,
        budget: ExecutionBudget,
        trace: &mut dyn TraceSink,
    ) -> Result<ExitReason, InterpreterError> {
        if let Some(fault) = thread.pending_fault {
            return Ok(ExitReason::Fault(fault));
        }
        for _ in 0..budget.instructions {
            match self.step(thread, memory, trace)? {
                StepOutcome::Continue(_) => {}
                StepOutcome::Dispatch(pc) => return Ok(ExitReason::Dispatch(pc)),
                StepOutcome::Fault(fault) => return Ok(ExitReason::Fault(fault)),
            }
        }
        Ok(ExitReason::BudgetExhausted)
    }

    fn terminate(
        &mut self,
        block: &IrBlock,
        thread: &mut GuestThread,
        trace: &mut dyn TraceSink,
    ) -> Result<StepOutcome, InterpreterError> {
        let edge = match &block.exit {
            Exit::Jump(edge) => edge,
            Exit::Branch { condition, yes, no } => {
                if data(&self.locals, *condition).bits == 0 {
                    no
                } else {
                    yes
                }
            }
            Exit::NextPc(value) => {
                let pc = GuestAddress(data(&self.locals, *value).bits as u64);
                thread.cpu.set_pc(pc);
                self.finished = true;
                return Ok(StepOutcome::Dispatch(pc));
            }
            Exit::Fault(class) => return Ok(self.trap(thread, trace, *class)),
            Exit::Trap { code } => return Ok(self.trap(thread, trace, FaultClass::Trap(*code))),
            _ => {
                self.finished = true;
                return Err(InterpreterError::UnsupportedIr);
            }
        };
        Ok(self.follow(edge, thread))
    }

    fn follow(&mut self, edge: &Edge, thread: &mut GuestThread) -> StepOutcome {
        self.block = self.blocks[&edge.target];
        self.offset = 0;
        let block = &self.function.as_ref().blocks[self.block];
        self.locals = block
            .parameters
            .iter()
            .zip(&edge.args)
            .map(|(param, &arg)| (param.value, Local::Data(data(&self.locals, arg))))
            .collect();
        let pc = next_pc(block, 0);
        thread.cpu.set_pc(pc);
        StepOutcome::Continue(pc)
    }

    fn trap(
        &mut self,
        thread: &mut GuestThread,
        trace: &mut dyn TraceSink,
        class: FaultClass,
    ) -> StepOutcome {
        self.fault(
            thread,
            trace,
            GuestFault {
                pc: thread.cpu.pc(),
                thread: thread.id,
                class,
                memory_access: None,
            },
        )
    }

    fn fault(
        &mut self,
        thread: &mut GuestThread,
        trace: &mut dyn TraceSink,
        fault: GuestFault,
    ) -> StepOutcome {
        self.finished = true;
        thread.pending_fault = Some(fault);
        trace.record(TraceEvent::Fault(fault));
        StepOutcome::Fault(fault)
    }
}

fn next_pc(block: &IrBlock, offset: usize) -> GuestAddress {
    block
        .instructions
        .get(offset)
        .map_or(block.exit_pc, |inst| inst.pc)
}

fn data(locals: &Locals, id: ValueId) -> Value {
    match locals[&id] {
        Local::Data(value) => value,
        Local::Guard(_) => unreachable!("verified data operand"),
    }
}

fn guarded_address(locals: &Locals, id: ValueId) -> GuestAddress {
    match locals[&id] {
        Local::Guard(address) => address,
        Local::Data(_) => unreachable!("verified guard operand"),
    }
}
