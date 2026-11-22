use bifrost_core::{
    AbiId, CpuState, FaultClass, GuestAddress as Addr, GuestIsa, GuestOs, GuestTarget, GuestThread,
    GuestThreadId, TraceEvent, TraceSink, X86State,
};
use bifrost_executor::{ExecutionBudget, ExitReason};
use bifrost_interpreter::{Interpreter, InterpreterError, StepOutcome};
use bifrost_ir::*;
use bifrost_memory::AddressSpace;
use bifrost_testkit::{scalar_ir, scalar_memory};

#[derive(Default)]
struct Trace(Vec<TraceEvent>);
impl TraceSink for Trace {
    fn record(&mut self, event: TraceEvent) {
        self.0.push(event);
    }
}

fn thread() -> GuestThread {
    GuestThread {
        id: GuestThreadId(3),
        cpu: CpuState::X86_64(Box::new(X86State {
            rip: Addr(0x1000),
            ..X86State::default()
        })),
        pending_fault: None,
    }
}

#[test]
fn executes_the_starter_fixture_as_one_guest_instruction() {
    let function = scalar_ir().verify().unwrap();
    let mut interpreter = Interpreter::new(&function, &[]).unwrap();
    let mut thread = thread();
    let mut memory = scalar_memory().unwrap();
    let mut trace = Trace::default();
    assert_eq!(
        interpreter
            .step(&mut thread, &mut memory, &mut trace)
            .unwrap(),
        StepOutcome::Dispatch(Addr(0x1001))
    );
    let CpuState::X86_64(state) = &thread.cpu else {
        panic!("expected x86 state")
    };
    assert_eq!(state.gpr[0], 42);
    assert_eq!(state.rip, Addr(0x1001));
    assert_eq!(
        trace.0,
        [TraceEvent::Step {
            thread: thread.id,
            pc: Addr(0x1000)
        }]
    );
    assert_eq!(
        interpreter.step(&mut thread, &mut memory, &mut trace),
        Err(InterpreterError::Finished)
    );
}

#[test]
fn bounds_a_loop_and_resumes_with_block_arguments() {
    let target = GuestTarget::new(
        GuestIsa::X86_64,
        GuestOs::None,
        AbiId::Bare,
        Default::default(),
    )
    .unwrap();
    let function = IrFunction {
        target,
        entry: BlockId(8),
        blocks: vec![IrBlock {
            id: BlockId(8),
            span: CodeSpan {
                start: Addr(0x1000),
                len: 2,
            },
            parameters: vec![Parameter {
                value: ValueId(0),
                ty: Type::I64,
            }],
            instructions: vec![
                Instruction {
                    pc: Addr(0x1000),
                    op: Op::Const {
                        out: ValueId(1),
                        ty: Type::I64,
                        bits: 1,
                    },
                },
                Instruction {
                    pc: Addr(0x1000),
                    op: Op::Int {
                        out: ValueId(2),
                        op: IntOp::Add,
                        lhs: ValueId(0),
                        rhs: ValueId(1),
                    },
                },
                Instruction {
                    pc: Addr(0x1001),
                    op: Op::WriteRegister {
                        register: Register::X86(X86Reg::Gpr(0)),
                        value: ValueId(2),
                    },
                },
            ],
            exit_pc: Addr(0x1001),
            exit: Exit::Jump(Edge {
                target: BlockId(8),
                args: vec![ValueId(2)],
            }),
        }],
    }
    .verify()
    .unwrap();
    let mut engine = Interpreter::new(&function, &[u64::MAX.into()]).unwrap();
    let mut thread = thread();
    let mut memory = AddressSpace::new(4096, 0).unwrap();
    let mut trace = Trace::default();
    for (budget, pc) in [(0, Addr(0x1000)), (1, Addr(0x1001)), (3, Addr(0x1000))] {
        assert_eq!(
            engine
                .run(
                    &mut thread,
                    &mut memory,
                    ExecutionBudget {
                        instructions: budget
                    },
                    &mut trace
                )
                .unwrap(),
            ExitReason::BudgetExhausted
        );
        assert_eq!(thread.cpu.pc(), pc);
    }
    let CpuState::X86_64(state) = &thread.cpu else {
        panic!("expected x86 state")
    };
    assert_eq!(state.gpr[0], 1);
    assert_eq!(trace.0.len(), 4);
}

#[test]
fn suspends_on_a_fault_at_the_guest_pc() {
    let function = scalar_ir().verify().unwrap();
    let mut engine = Interpreter::new(&function, &[]).unwrap();
    let mut memory = AddressSpace::new(4096, 0).unwrap();
    let mut thread = thread();
    let mut trace = Trace::default();
    let StepOutcome::Fault(fault) = engine.step(&mut thread, &mut memory, &mut trace).unwrap()
    else {
        panic!("expected unmapped access")
    };
    assert_eq!(fault.class, FaultClass::Unmapped);
    assert_eq!(fault.pc, Addr(0x1000));
    assert_eq!(thread.cpu.pc(), fault.pc);
    assert_eq!(thread.pending_fault, Some(fault));
    assert_eq!(
        engine.step(&mut thread, &mut memory, &mut trace).unwrap(),
        StepOutcome::Fault(fault)
    );
    assert_eq!(
        trace.0,
        [
            TraceEvent::Step {
                thread: thread.id,
                pc: fault.pc
            },
            TraceEvent::Fault(fault)
        ]
    );
}
