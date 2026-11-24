#![cfg(all(feature = "guest-aarch64", feature = "interp"))]

use bifrost_runtime::{
    core::{
        AbiId, AccessKind, Arm64State, CpuFeature, CpuState, FaultClass, GuestAddress as Addr,
        GuestIsa, GuestOs, GuestTarget, GuestThread, GuestThreadId, TraceEvent, TraceSink,
    },
    memory::{AddressSpace, GuestMemory, GuestMemoryRead, Protection},
    raw::{ExecutionBudget, ExecutionError, Executor, ExitReason, RawInterpreter},
};
use bifrost_testkit::arm64::{MOVE_ADD_AND_CALL, MOVE_ADD_AND_CALL_STEPS, bytes};

const CODE: Addr = Addr(0x1000);
const DATA: Addr = Addr(0x2000);

fn target() -> GuestTarget {
    GuestTarget::new(
        GuestIsa::AArch64,
        GuestOs::None,
        AbiId::Bare,
        Default::default(),
    )
    .unwrap()
}

fn memory_and_thread(words: &[u32], data: Protection) -> (AddressSpace, GuestThread) {
    let mut memory = AddressSpace::new(4096, 2).unwrap();
    memory.map(CODE, 4096, Protection::ReadWrite).unwrap();
    memory.write(CODE, &bytes(words)).unwrap();
    memory.protect(CODE, 4096, Protection::ReadExecute).unwrap();
    memory.map(DATA, 4096, data).unwrap();
    let mut state = Arm64State {
        pc: CODE,
        sp: Addr(0x3000),
        ..Default::default()
    };
    state.x[1] = DATA.0;
    (
        memory,
        GuestThread {
            id: GuestThreadId(1),
            cpu: CpuState::AArch64(Box::new(state)),
            pending_fault: None,
        },
    )
}

#[derive(Default)]
struct Trace(Vec<TraceEvent>);
impl TraceSink for Trace {
    fn record(&mut self, event: TraceEvent) {
        self.0.push(event);
    }
}

fn state(thread: &GuestThread) -> &Arm64State {
    let CpuState::AArch64(state) = &thread.cpu else {
        panic!("ARM64 fixture state");
    };
    state
}

#[test]
fn interprets_memory_flags_link_and_barrier() {
    let (mut memory, mut thread) = memory_and_thread(&MOVE_ADD_AND_CALL, Protection::ReadWrite);
    let mut trace = Trace::default();
    let reason = RawInterpreter::new(&target(), &mut memory, &mut trace)
        .unwrap()
        .run(
            &mut thread,
            ExecutionBudget {
                instructions: MOVE_ADD_AND_CALL_STEPS,
            },
        )
        .unwrap();
    assert_eq!(reason, ExitReason::BudgetExhausted);
    assert_eq!(state(&thread).x[0], 43);
    assert_eq!(state(&thread).x[2], 41);
    assert_eq!(state(&thread).x[30], CODE.0 + 32);
    assert_eq!(state(&thread).nzcv, 0x6000_0000);
    assert_eq!(thread.cpu.pc(), Addr(CODE.0 + 52));
    assert_eq!(trace.0.len(), MOVE_ADD_AND_CALL_STEPS as usize);
    let mut data = [0; 8];
    memory.read(Addr(DATA.0 + 8), &mut data).unwrap();
    assert_eq!(u64::from_le_bytes(data), 41);
}

#[test]
fn reports_precise_fetch_and_data_faults_and_rejects_unsupported_instructions() {
    let (mut memory, mut thread) = memory_and_thread(&[0xf940_0420], Protection::Guard);
    let previous = thread.cpu.clone();
    let ExitReason::Fault(fault) = RawInterpreter::new(&target(), &mut memory, &mut ())
        .unwrap()
        .step(&mut thread)
        .unwrap()
    else {
        panic!("expected guarded read");
    };
    assert_eq!(fault.pc, CODE);
    assert_eq!(fault.class, FaultClass::Guard);
    assert_eq!(
        fault.memory_access,
        Some((Addr(DATA.0 + 8), AccessKind::Read))
    );
    assert_eq!(thread.cpu, previous);

    let (mut memory, mut thread) = memory_and_thread(&[0xd400_0002], Protection::ReadWrite);
    let error = RawInterpreter::new(&target(), &mut memory, &mut ())
        .unwrap()
        .step(&mut thread)
        .unwrap_err();
    assert!(matches!(
        error,
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::UnsupportedInstruction {
            pc: Addr(0x1000),
            word: 0xd400_0002,
            ..
        })
    ));
    assert!(thread.pending_fault.is_none());
    thread.cpu.set_pc(Addr(CODE.0 + 1));
    let ExitReason::Fault(fault) = RawInterpreter::new(&target(), &mut memory, &mut ())
        .unwrap()
        .step(&mut thread)
        .unwrap()
    else {
        panic!("expected unaligned fetch");
    };
    assert_eq!(fault.class, FaultClass::Alignment);
    assert_eq!(fault.pc, Addr(CODE.0 + 1));
    assert_eq!(fault.memory_access, Some((fault.pc, AccessKind::Execute)));
}

#[test]
fn supervisor_call_exits_without_changing_guest_state() {
    const IMMEDIATE: u16 = 0x8123;
    let call = 0xd400_0001 | (u32::from(IMMEDIATE) << 5);
    let (mut memory, mut thread) = memory_and_thread(&[call], Protection::ReadWrite);
    let previous = thread.cpu.clone();
    let expected = ExitReason::Arm64SupervisorCall {
        pc: CODE,
        immediate: IMMEDIATE,
    };
    let mut trace = Trace::default();
    let budget = ExecutionBudget { instructions: 1 };
    let observed = RawInterpreter::new(&target(), &mut memory, &mut trace)
        .unwrap()
        .run(&mut thread, budget)
        .unwrap();
    assert_eq!(observed, expected);
    assert_eq!(thread.cpu, previous);
    assert!(thread.pending_fault.is_none());
    assert_eq!(
        trace.0,
        [TraceEvent::Step {
            thread: thread.id,
            pc: CODE
        }]
    );

    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        use bifrost_runtime::raw::{RawJit, SystemJitMemory};
        let mut jit_trace = Trace::default();
        let observed = RawJit::new(&target(), &mut memory, &mut jit_trace, SystemJitMemory)
            .unwrap()
            .run(&mut thread, budget)
            .unwrap();
        assert_eq!(observed, expected);
        assert_eq!(thread.cpu, previous);
        assert!(thread.pending_fault.is_none());
        assert_eq!(jit_trace.0, trace.0);
    }
}

#[test]
fn word_moves_zero_extend_without_setting_flags() {
    let code = [
        0x2a01_03e0, // mov w0,w1
        0x2a1f_03e2, // mov w2,wzr
        0x2a01_03ff, // mov wzr,w1
        0x52a2_4683, // movz w3,#0x1234,lsl #16
        0x1280_0004, // movn w4,#0
    ];
    let (mut memory, mut thread) = memory_and_thread(&code, Protection::ReadWrite);
    let CpuState::AArch64(cpu) = &mut thread.cpu else {
        unreachable!()
    };
    cpu.x[0] = u64::MAX;
    cpu.x[1] = 0x1234_5678_fedc_ba98;
    cpu.x[2] = u64::MAX;
    cpu.nzcv = 0xa000_0000;
    let original = thread.clone();
    let budget = ExecutionBudget {
        instructions: code.len() as u64,
    };
    let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
        .unwrap()
        .run(&mut thread, budget)
        .unwrap();
    assert_eq!(exit, ExitReason::BudgetExhausted);
    let result = state(&thread);
    assert_eq!(
        result.x[..5],
        [
            0xfedc_ba98,
            0x1234_5678_fedc_ba98,
            0,
            0x1234_0000,
            0xffff_ffff
        ]
    );
    assert_eq!(result.nzcv, 0xa000_0000);
    assert_eq!(result.sp, state(&original).sp);

    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        use bifrost_runtime::raw::{RawJit, SystemJitMemory};
        let mut jitted = original;
        let actual = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
            .unwrap()
            .run(&mut jitted, budget)
            .unwrap();
        assert_eq!(actual, exit);
        assert_eq!(jitted, thread);
    }
}

#[test]
fn byte_memory_access_preserves_adjacent_bytes_and_zero_extends_loads() {
    check_byte_memory_access([0x3900_0420, 0x3940_0422], DATA.0); // strb/ldrb [x1,#1]
    check_byte_memory_access([0x381f_f020, 0x385f_f022], DATA.0 + 2); // sturb/ldurb [x1,#-1]
}

fn check_byte_memory_access(code: [u32; 2], base: u64) {
    let (mut memory, mut thread) = memory_and_thread(&code, Protection::ReadWrite);
    memory.write(DATA, &[1, 2, 3]).unwrap();
    let CpuState::AArch64(cpu) = &mut thread.cpu else {
        unreachable!()
    };
    cpu.x[0] = 0x1234_56ab;
    cpu.x[1] = base;
    cpu.x[2] = u64::MAX;
    let original = thread.clone();
    let budget = ExecutionBudget { instructions: 2 };
    let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
        .unwrap()
        .run(&mut thread, budget)
        .unwrap();
    assert_eq!(exit, ExitReason::BudgetExhausted);
    let mut bytes = [0; 3];
    memory.read(DATA, &mut bytes).unwrap();
    assert_eq!(bytes, [1, 0xab, 3]);
    assert_eq!(state(&thread).x[2], 0xab);
    assert_eq!(state(&thread).x[0], state(&original).x[0]);

    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        use bifrost_runtime::raw::{RawJit, SystemJitMemory};
        memory.write(DATA, &[1, 2, 3]).unwrap();
        let mut jitted = original;
        let actual = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
            .unwrap()
            .run(&mut jitted, budget)
            .unwrap();
        assert_eq!(actual, exit);
        assert_eq!(jitted, thread);
        memory.read(DATA, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 0xab, 3]);
    }
}

#[test]
fn validates_arm64_feature_requirements_at_construction() {
    let (mut memory, _) = memory_and_thread(&[0xd503_201f], Protection::ReadWrite);
    for feature in [CpuFeature::Neon, CpuFeature::Lse] {
        let target = GuestTarget::new(
            GuestIsa::AArch64,
            GuestOs::None,
            AbiId::Bare,
            [feature].into_iter().collect(),
        )
        .unwrap();
        assert!(matches!(
            RawInterpreter::new(&target, &mut memory, &mut ()),
            Err(ExecutionError::UnsupportedTarget(_))
        ));
    }
}

#[cfg(all(
    feature = "jit-cranelift",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn arm64_jit_agrees_on_guest_state_faults_and_trace() {
    use bifrost_runtime::raw::{RawJit, SystemJitMemory};

    for (program, count, protection, return_address) in [
        (
            &MOVE_ADD_AND_CALL[..],
            MOVE_ADD_AND_CALL_STEPS,
            Protection::ReadWrite,
            0,
        ),
        (&[0xf900_0420][..], 1, Protection::Read, 0),
        (&[0xf940_0420][..], 1, Protection::Guard, 0),
        (&[0x0000_0000][..], 1, Protection::ReadWrite, 0),
        (&[0xd65f_03c0][..], 2, Protection::ReadWrite, CODE.0 + 1),
        (&[0xd65f_03c0][..], 2, Protection::ReadWrite, 0x6000),
    ] {
        let (mut interpreted_memory, mut interpreted_thread) =
            memory_and_thread(program, protection);
        if let CpuState::AArch64(cpu) = &mut interpreted_thread.cpu {
            cpu.x[30] = return_address;
        }
        let mut jitted_thread = interpreted_thread.clone();
        let mut jitted_memory = memory_and_thread(program, protection).0;
        let mut jitted_trace = Trace::default();
        let mut interpreted_trace = Trace::default();
        let budget = ExecutionBudget {
            instructions: count,
        };
        let expected =
            RawInterpreter::new(&target(), &mut interpreted_memory, &mut interpreted_trace)
                .unwrap()
                .run(&mut interpreted_thread, budget)
                .unwrap();
        let actual = RawJit::new(
            &target(),
            &mut jitted_memory,
            &mut jitted_trace,
            SystemJitMemory,
        )
        .unwrap()
        .run(&mut jitted_thread, budget)
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(jitted_thread, interpreted_thread);
        assert_eq!(jitted_trace.0, interpreted_trace.0);
        if protection == Protection::ReadWrite {
            let (mut expected, mut actual) = ([0; 16], [0; 16]);
            interpreted_memory.read(DATA, &mut expected).unwrap();
            jitted_memory.read(DATA, &mut actual).unwrap();
            assert_eq!(actual, expected);
        }
    }

    let (mut memory, mut thread) = memory_and_thread(&[0xd400_0002], Protection::ReadWrite);
    let before = thread.clone();
    let error = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
        .unwrap()
        .step(&mut thread)
        .unwrap_err();
    assert!(matches!(
        error,
        ExecutionError::Arm64Lift(bifrost_lifter_arm64::LiftError::UnsupportedInstruction {
            pc: CODE,
            word: 0xd400_0002,
            ..
        })
    ));
    assert_eq!(thread, before);
}

#[cfg(all(
    feature = "jit-cranelift",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn arm64_cache_checks_all_code_page_generations() {
    use bifrost_runtime::raw::{RawJit, SystemJitMemory};

    let mut memory = AddressSpace::new(2, 2).unwrap();
    memory.map(CODE, 4, Protection::ReadWrite).unwrap();
    memory.write(CODE, &0xd280_0020_u32.to_le_bytes()).unwrap(); // MOVZ X0,#1
    memory.protect(CODE, 4, Protection::Execute).unwrap();
    let second_page = Addr(CODE.0 + 2);
    let mut trace = ();
    let mut jit = RawJit::new(&target(), &mut memory, &mut trace, SystemJitMemory).unwrap();
    let mut thread = memory_and_thread(&[], Protection::None).1;
    for _ in 0..2 {
        thread.cpu.set_pc(CODE);
        assert_eq!(
            jit.step(&mut thread).unwrap(),
            ExitReason::Dispatch(Addr(CODE.0 + 4))
        );
        assert_eq!(state(&thread).x[0], 1);
    }
    assert_eq!(jit.cache_stats().hits, 1);
    jit.memory_mut()
        .protect(second_page, 2, Protection::ReadWrite)
        .unwrap();
    jit.memory_mut()
        .write(second_page, &0x9280_u16.to_le_bytes())
        .unwrap(); // MOVN X0,#1
    jit.memory_mut()
        .protect(second_page, 2, Protection::Execute)
        .unwrap();
    thread.cpu.set_pc(CODE);
    assert_eq!(
        jit.step(&mut thread).unwrap(),
        ExitReason::Dispatch(Addr(CODE.0 + 4))
    );
    assert_eq!(state(&thread).x[0], u64::MAX - 1);
    assert_eq!(jit.cache_stats().misses, 2);
}
