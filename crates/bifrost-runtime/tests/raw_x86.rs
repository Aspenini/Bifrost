#![cfg(all(feature = "guest-x86_64", feature = "interp"))]

use bifrost_runtime::{
    core::{
        AccessKind, CpuState, FaultClass, GuestAddress as Addr, GuestThread, GuestThreadId,
        TraceEvent, TraceSink, X86State,
    },
    memory::{AddressSpace, GuestMemory, GuestMemoryRead, Protection},
    x86::{ExecutionBudget, ExecutionError, Executor, ExitReason, X86Interpreter},
};
use bifrost_testkit::x86::{ARITHMETIC, ARITHMETIC_FLAGS, CALL_AND_RETURN, MOVE_AND_ADD};

const ENTRY: Addr = Addr(0x1000);

fn fixture(bytes: &[u8]) -> (AddressSpace, GuestThread) {
    let mut memory = AddressSpace::new(4096, 2).unwrap();
    memory.map(ENTRY, 4096, Protection::ReadWrite).unwrap();
    memory.write(ENTRY, bytes).unwrap();
    memory
        .protect(ENTRY, 4096, Protection::ReadExecute)
        .unwrap();
    let cpu = X86State {
        rip: ENTRY,
        ..X86State::default()
    };
    (
        memory,
        GuestThread {
            id: GuestThreadId(1),
            cpu: CpuState::X86_64(Box::new(cpu)),
            pending_fault: None,
        },
    )
}

fn cpu(thread: &mut GuestThread) -> &mut X86State {
    let CpuState::X86_64(cpu) = &mut thread.cpu else {
        panic!("expected x86-64");
    };
    cpu
}

#[derive(Default)]
struct Trace(Vec<TraceEvent>);
impl TraceSink for Trace {
    fn record(&mut self, event: TraceEvent) {
        self.0.push(event);
    }
}

#[test]
fn executes_all_arithmetic_vectors_and_preserves_unaffected_flags() {
    for case in ARITHMETIC {
        let (mut memory, mut thread) = fixture(case.bytes);
        let state = cpu(&mut thread);
        state.gpr[0] = case.lhs;
        state.gpr[3] = case.rhs;
        state.rflags = 0x602 | ARITHMETIC_FLAGS;
        assert_eq!(
            X86Interpreter::new(&mut memory, &mut ())
                .step(&mut thread)
                .unwrap(),
            ExitReason::Dispatch(Addr(ENTRY.0 + case.bytes.len() as u64)),
            "{}",
            case.name
        );
        assert_eq!(cpu(&mut thread).gpr[0], case.rax, "{}", case.name);
        assert_eq!(cpu(&mut thread).rflags, 0x602 | case.flags, "{}", case.name);
    }
}

#[test]
fn runs_memory_and_stack_programs_with_deterministic_pcs() {
    for (code, expected_rax, pcs, stack_value) in [
        (MOVE_AND_ADD, 42, [0, 10, 15, 20], 41),
        (CALL_AND_RETURN, 1, [0, 7, 11, 5], ENTRY.0 + 5),
    ] {
        let (mut memory, mut thread) = fixture(code);
        memory
            .map(Addr(0x2000), 4096, Protection::ReadWrite)
            .unwrap();
        cpu(&mut thread).gpr[4] = 0x3000;
        let mut trace = Trace::default();
        let reason = X86Interpreter::new(&mut memory, &mut trace)
            .run(&mut thread, ExecutionBudget { instructions: 4 })
            .unwrap();
        assert_eq!(reason, ExitReason::BudgetExhausted);
        assert_eq!(cpu(&mut thread).gpr[0], expected_rax);
        assert_eq!(thread.cpu.sp(), Addr(0x3000));
        assert_eq!(thread.cpu.pc(), Addr(ENTRY.0 + code.len() as u64));
        assert_eq!(
            trace.0,
            pcs.map(|offset| TraceEvent::Step {
                thread: thread.id,
                pc: Addr(ENTRY.0 + offset)
            })
        );
        let mut bytes = [0; 8];
        memory.read(Addr(0x2ff8), &mut bytes).unwrap();
        assert_eq!(u64::from_le_bytes(bytes), stack_value);
    }
}

#[test]
fn reports_precise_faults_and_preserves_faulting_instruction_state() {
    for (bytes, stack, class, memory_access) in [
        (
            &[0xe8, 0, 0, 0, 0][..],
            0x3000,
            FaultClass::Unmapped,
            Some((Addr(0x2ff8), AccessKind::Write)),
        ),
        (
            &[0xc3][..],
            0x3000,
            FaultClass::Unmapped,
            Some((Addr(0x3000), AccessKind::Read)),
        ),
        (&[0x0f, 0x0b][..], 0, FaultClass::IllegalInstruction, None),
        (&[0x66; 15][..], 0, FaultClass::IllegalInstruction, None),
    ] {
        let (mut memory, mut thread) = fixture(bytes);
        cpu(&mut thread).gpr[4] = stack;
        let checkpoint = thread.cpu.clone();
        let mut trace = Trace::default();
        let ExitReason::Fault(fault) = X86Interpreter::new(&mut memory, &mut trace)
            .step(&mut thread)
            .unwrap()
        else {
            panic!("expected fault");
        };
        assert_eq!(fault.pc, ENTRY);
        assert_eq!(fault.class, class);
        assert_eq!(fault.memory_access, memory_access);
        assert_eq!(checkpoint, thread.cpu);
        assert_eq!(thread.pending_fault, Some(fault));
        assert_eq!(
            trace.0,
            [
                TraceEvent::Step {
                    thread: thread.id,
                    pc: ENTRY
                },
                TraceEvent::Fault(fault)
            ]
        );
    }

    let (mut memory, mut thread) = fixture(&[0x0f, 0x05]);
    assert!(matches!(
        X86Interpreter::new(&mut memory, &mut ()).step(&mut thread),
        Err(ExecutionError::Lift(_))
    ));
    assert_eq!(thread.pending_fault, None);
    assert_eq!(thread.cpu.pc(), ENTRY);
}

#[test]
fn rejects_noncanonical_return_target_before_advancing_stack() {
    let (mut memory, mut thread) = fixture(&[0xc3]);
    memory
        .map(Addr(0x2000), 4096, Protection::ReadWrite)
        .unwrap();
    let address = 0x0000_8000_0000_0000_u64;
    memory.write(Addr(0x2000), &address.to_le_bytes()).unwrap();
    cpu(&mut thread).gpr[4] = 0x2000;
    let ExitReason::Fault(fault) = X86Interpreter::new(&mut memory, &mut ())
        .step(&mut thread)
        .unwrap()
    else {
        panic!("expected fault");
    };
    assert_eq!(fault.class, FaultClass::NonCanonicalAddress);
    assert_eq!(
        fault.memory_access,
        Some((Addr(address), AccessKind::Execute))
    );
    assert_eq!(thread.cpu.sp(), Addr(0x2000));
    assert_eq!(thread.cpu.pc(), ENTRY);
}
