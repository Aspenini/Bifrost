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
fn floating_point_and_vector_lanes_match_on_both_backends() {
    let execute = |#[allow(unused_variables)] jit: bool| {
        let (mut memory, mut thread) = memory_and_thread(
            &[
                0x1f41_0800,
                0x1f04_0863,
                0x1e62_0261,
                0x1e61_2805,
                0x6f07_e5c6,
                0x4d00_8026,
                0x4ddf_9027,
                0x6ea0_f8e9,
                0x138c_4d8c, // ror w12,w12,#19 (EXTR alias)
                0x138c_000d, // extr w13,w0,w12,#0
                0x1e20_40e8, // fmov s8,s7
                0x1e60_40eb, // fmov d11,d7
                0x1e22_c10e, // fcvt d14,s8
                0x1e62_40af, // fcvt s15,d5
                0x1e21_c1f0, // fsqrt s16,s15 (negative input sets invalid)
                0x1e64_0420, // fccmp d1,d4,#0,eq
                0x1f41_8436, // fmsub d22,d1,d1,d1
                0x1f61_0437, // fnmadd d23,d1,d1,d1
                0x1f61_8438, // fnmsub d24,d1,d1,d1
                0xd53b_442a, // mrs x10,fpsr
                0xd51b_4414, // msr fpcr,x20
                0xd53b_440b, // mrs x11,fpcr
                0xd51b_443f, // msr fpsr,xzr
                0x1e61_0b39, // fmul d25,d25,d1 (default NaN enabled)
                0xd53b_442c, // mrs x12,fpsr
                0x4e21_db5b, // scvtf v27.4s,v26.4s
                0x4ea1_bb7c, // fcvtzs v28.4s,v27.4s
                0x7e21_db5d, // ucvtf s29,s26
                0x9e38_036e, // fcvtzs x14,s27
            ],
            Protection::ReadWrite,
        );
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        let epsilon = 2.0f64.powi(-27);
        cpu.v[0] = u128::from((1.0 + epsilon).to_bits()) | (u128::MAX << 64);
        cpu.v[1] = u128::from((1.0 - epsilon).to_bits());
        cpu.v[2] = u128::from((-1.0f64).to_bits()) | (u128::MAX << 64);
        cpu.v[3] = u128::from(f32::INFINITY.to_bits());
        cpu.v[7] = 0xdead_beef_0123_4567_89ab_cdef_cafe_babe;
        cpu.v[25] = 0x7ff0_0000_0000_0001;
        cpu.v[26] = 0xffff_fffc_0000_0003_ffff_fffe_0000_0001;
        cpu.v[29] = u128::MAX;
        cpu.x[19] = 0x1234_5678_ffff_fffd;
        cpu.x[20] = 0xffff_ffff_0200_0040;
        cpu.x[12] = 0x1234_5678_9988_7766;
        // The binary32 instruction multiplies infinity by the zero in V4.
        cpu.fpsr = 1 << 27;
        cpu.nzcv = 0x4000_0000;
        let budget = ExecutionBudget { instructions: 29 };
        let mut trace = Trace::default();
        let interpret = |memory: &mut AddressSpace, thread: &mut GuestThread, trace: &mut Trace| {
            RawInterpreter::new(&target(), memory, trace)
                .unwrap()
                .run(thread, budget)
                .unwrap()
        };
        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        let stop = if jit {
            use bifrost_runtime::raw::{RawJit, SystemJitMemory};
            RawJit::new(&target(), &mut memory, &mut trace, SystemJitMemory)
                .unwrap()
                .run(&mut thread, budget)
                .unwrap()
        } else {
            interpret(&mut memory, &mut thread, &mut trace)
        };
        #[cfg(not(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        )))]
        let stop = interpret(&mut memory, &mut thread, &mut trace);
        assert_eq!(stop, ExitReason::BudgetExhausted);
        assert_eq!(trace.0.len(), 29);
        let cpu = state(&thread);
        assert_eq!(cpu.pc, Addr(CODE.0 + 116));
        assert_eq!(cpu.v[0], u128::from((-2.0f64.powi(-54)).to_bits()));
        assert_eq!(cpu.v[3], u128::from(f32::NAN.to_bits()));
        assert_eq!(cpu.v[1], u128::from((-3.0f64).to_bits()));
        assert_eq!(cpu.v[5], u128::from((-3.0f64).to_bits()));
        assert_eq!(cpu.v[6], 0xffff_ff00_ffff_ff00_ffff_ff00_ffff_ff00);
        assert_eq!(cpu.v[7], 0xffff_ff00_0123_4567_89ab_cdef_cafe_babe);
        assert_eq!(
            cpu.v[9],
            cpu.v[7] ^ 0x8000_0000_8000_0000_8000_0000_8000_0000
        );
        assert_eq!(cpu.x[1], DATA.0 + 4);
        assert_eq!(cpu.x[13], u64::from(0x9988_7766_u32.rotate_right(19)));
        assert_eq!(cpu.x[10], (1 << 27) | 0x11);
        assert_eq!(cpu.x[11], 1 << 25);
        assert_eq!(cpu.x[12], 1);
        assert_eq!(cpu.v[8], u128::from(cpu.v[7] as u32));
        assert_eq!(cpu.v[11], u128::from(cpu.v[7] as u64));
        assert_eq!(
            cpu.v[14],
            u128::from(f64::from(f32::from_bits(cpu.v[8] as u32)).to_bits())
        );
        assert_eq!(cpu.v[15], u128::from((-3.0f32).to_bits()));
        assert_eq!(cpu.v[16], u128::from(f32::NAN.to_bits()));
        assert_eq!(cpu.nzcv, 0x8000_0000);
        assert_eq!(cpu.v[22], u128::from((-12.0f64).to_bits()));
        assert_eq!(cpu.v[23], u128::from((-6.0f64).to_bits()));
        assert_eq!(cpu.v[24], u128::from(12.0f64.to_bits()));
        assert_eq!(cpu.v[25], u128::from(f64::NAN.to_bits()));
        assert_eq!(cpu.v[27], 0xc080_0000_4040_0000_c000_0000_3f80_0000);
        assert_eq!(cpu.v[28], cpu.v[26]);
        assert_eq!(cpu.v[29], u128::from(1.0f32.to_bits()));
        assert_eq!(cpu.x[14], 1);
        let mut lane = [0; 4];
        memory.read(DATA, &mut lane).unwrap();
        assert_eq!(u32::from_le_bytes(lane), 0xffff_ff00);
        assert_eq!(cpu.fpcr, 1 << 25);
        assert_eq!(cpu.fpsr, 1);
    };
    execute(false);
    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    execute(true);
}

#[test]
fn scalar_and_vector_transfers_preserve_bits_and_clear_unused_lanes() {
    let words = [
        0xfd40_0420, // ldr d0,[x1,#8]
        0xbd00_1420, // str s0,[x1,#20]
        0x3cc2_0022, // ldur q2,[x1,#32]
        0x6d42_1c26, // ldp d6,d7,[x1,#32]
        0x2d08_1c26, // stp s6,s7,[x1,#64]
        0x2d48_2c2a, // ldp s10,s11,[x1,#64]
        0x9e66_0008, // fmov x8,d0
        0x1e27_0104, // fmov s4,w8
        0x1e7f_1003, // fmov d3,#-1.5
        0x1e20_1005, // fmov s5,#2.0
        0x3c83_0c22, // str q2,[x1,#48]!
        0x4e20_584c, // cnt v12.16b,v2.16b
        0x0e20_5842, // cnt v2.8b,v2.8b
    ];
    let execute = |#[allow(unused_variables)] jit: bool| {
        let (mut memory, mut thread) = memory_and_thread(&words, Protection::ReadWrite);
        let value = 0x1234_5678_dead_beef_u64;
        let lanes = 0xdead_face_cafe_beef_8765_4321_0123_4567_u128;
        memory
            .write(Addr(DATA.0 + 8), &value.to_le_bytes())
            .unwrap();
        memory
            .write(Addr(DATA.0 + 32), &lanes.to_le_bytes())
            .unwrap();
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.v.fill(u128::MAX);
        let budget = ExecutionBudget {
            instructions: words.len() as u64,
        };
        let run_interpreter = |memory: &mut AddressSpace, thread: &mut GuestThread| {
            RawInterpreter::new(&target(), memory, &mut ())
                .unwrap()
                .run(thread, budget)
                .unwrap()
        };
        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        let stop = if jit {
            use bifrost_runtime::raw::{RawJit, SystemJitMemory};
            RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
                .unwrap()
                .run(&mut thread, budget)
                .unwrap()
        } else {
            run_interpreter(&mut memory, &mut thread)
        };
        #[cfg(not(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        )))]
        let stop = run_interpreter(&mut memory, &mut thread);

        assert_eq!(stop, ExitReason::BudgetExhausted);
        let cpu = state(&thread);
        assert_eq!(cpu.x[1], DATA.0 + 48);
        assert_eq!(cpu.x[8], value);
        assert_eq!(cpu.v[0], u128::from(value));
        assert_eq!(cpu.v[2], 0x0404_0302_0103_0305);
        assert_eq!(cpu.v[3], u128::from((-1.5f64).to_bits()));
        assert_eq!(cpu.v[4], u128::from(value as u32));
        assert_eq!(cpu.v[5], u128::from(2f32.to_bits()));
        assert_eq!(cpu.v[6], u128::from(lanes as u64));
        assert_eq!(cpu.v[7], lanes >> 64);
        assert_eq!(cpu.v[10], u128::from(lanes as u32));
        assert_eq!(cpu.v[11], u128::from((lanes >> 64) as u32));
        assert_eq!(cpu.v[12], 0x0605_0605_0407_0607_0404_0302_0103_0305);
        let mut scalar = [0; 4];
        memory.read(Addr(DATA.0 + 20), &mut scalar).unwrap();
        assert_eq!(scalar, (value as u32).to_le_bytes());
        let mut vector = [0; 16];
        memory.read(Addr(DATA.0 + 48), &mut vector).unwrap();
        assert_eq!(vector, lanes.to_le_bytes());
    };
    execute(false);
    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    execute(true);
}

#[test]
fn vector_pair_transfers_preserve_all_bits_and_fault_before_writeback() {
    let execute = |#[allow(unused_variables)] jit: bool, crossing_page: bool| {
        let words = if crossing_page {
            [0xac81_1c26, 0xd503_201f] // stp q6,q7,[x1],#32; nop
        } else {
            [0xadbf_1fe6, 0xacc1_7fe8] // stp q6,q7,[sp,#-32]!; ldp q8,q31,[sp],#32
        };
        let (mut memory, mut thread) = memory_and_thread(&words, Protection::ReadWrite);
        let end = DATA.0 + 4096;
        let first = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210_u128;
        let second = 0xabba_aaba_0000_0012_ffee_0042_6677_8899_u128;
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.v[6] = first;
        cpu.v[7] = second;
        cpu.x[1] = end - 16;
        memory.write(Addr(end - 32), &[0x55; 32]).unwrap();
        let budget = ExecutionBudget { instructions: 2 };
        let run_interpreter = |memory: &mut AddressSpace, thread: &mut GuestThread| {
            RawInterpreter::new(&target(), memory, &mut ())
                .unwrap()
                .run(thread, budget)
                .unwrap()
        };
        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        let stop = if jit {
            use bifrost_runtime::raw::{RawJit, SystemJitMemory};
            RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
                .unwrap()
                .run(&mut thread, budget)
                .unwrap()
        } else {
            run_interpreter(&mut memory, &mut thread)
        };
        #[cfg(not(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        )))]
        let stop = run_interpreter(&mut memory, &mut thread);

        let mut data = [0; 32];
        memory.read(Addr(end - 32), &mut data).unwrap();
        let cpu = state(&thread);
        if crossing_page {
            let ExitReason::Fault(fault) = stop else {
                panic!("expected guest memory fault")
            };
            assert_eq!(fault.memory_access, Some((Addr(end), AccessKind::Write)));
            assert_eq!(cpu.pc, CODE);
            assert_eq!(cpu.x[1], end - 16);
            assert_eq!(data, [0x55; 32]);
        } else {
            assert_eq!(stop, ExitReason::BudgetExhausted);
            assert_eq!(cpu.sp, Addr(end));
            assert_eq!(cpu.v[8], first);
            assert_eq!(cpu.v[31], second);
            assert_eq!(&data[..16], &first.to_le_bytes());
            assert_eq!(&data[16..], &second.to_le_bytes());
        }
    };
    for crossing_page in [false, true] {
        execute(false, crossing_page);
        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        execute(true, crossing_page);
    }
}

#[test]
fn signed_immediate_loads_extend_to_the_destination_width() {
    let words = [
        0x3980_0020, // ldrsb x0,[x1]
        0x79c0_0422, // ldrsh w2,[x1,#2]
        0xb980_0423, // ldrsw x3,[x1,#4]
        0x78c0_3024, // ldursh w4,[x1,#3]
    ];
    let execute = |#[allow(unused_variables)] jit: bool| {
        let (mut memory, mut thread) = memory_and_thread(&words, Protection::ReadWrite);
        memory
            .write(DATA, &[0xfd, 0, 0xfd, 0xff, 0xfc, 0xff, 0xff, 0xff])
            .unwrap();
        let budget = ExecutionBudget {
            instructions: words.len() as u64,
        };
        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        if jit {
            use bifrost_runtime::raw::{RawJit, SystemJitMemory};
            let stop = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
                .unwrap()
                .run(&mut thread, budget)
                .unwrap();
            assert_eq!(stop, ExitReason::BudgetExhausted);
            return thread;
        }
        let stop = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .run(&mut thread, budget)
            .unwrap();
        assert_eq!(stop, ExitReason::BudgetExhausted);
        thread
    };
    let thread = execute(false);
    assert_eq!(
        state(&thread).x[..5],
        [u64::MAX - 2, DATA.0, 0xffff_fffd, u64::MAX - 3, 0xffff_fcff]
    );
    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    assert_eq!(execute(true).cpu, thread.cpu);
}

#[test]
fn generic_timer_reads_exit_without_changing_guest_state() {
    use bifrost_runtime::core::Arm64CounterRegister;

    for (word, register, rt) in [
        (0xd53b_e007, Arm64CounterRegister::Frequency, 7),
        (0xd53b_e020, Arm64CounterRegister::PhysicalCount, 0),
        (0xd53b_e05f, Arm64CounterRegister::VirtualCount, 31),
    ] {
        let (mut memory, mut thread) = memory_and_thread(&[word], Protection::ReadWrite);
        let previous = thread.cpu.clone();
        let expected = ExitReason::Arm64ReadCounter {
            pc: CODE,
            register,
            rt,
        };
        let observed = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .step(&mut thread)
            .unwrap();
        assert_eq!(observed, expected);
        assert_eq!(thread.cpu, previous);
        assert!(thread.pending_fault.is_none());

        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            use bifrost_runtime::raw::{RawJit, SystemJitMemory};
            let observed = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
                .unwrap()
                .step(&mut thread)
                .unwrap();
            assert_eq!(observed, expected);
            assert_eq!(thread.cpu, previous);
            assert!(thread.pending_fault.is_none());
        }
    }
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
        0x72b7_dde5, // movk w5,#0xbeef,lsl #16
    ];
    let (mut memory, mut thread) = memory_and_thread(&code, Protection::ReadWrite);
    let CpuState::AArch64(cpu) = &mut thread.cpu else {
        unreachable!()
    };
    cpu.x[0] = u64::MAX;
    cpu.x[1] = 0x1234_5678_fedc_ba98;
    cpu.x[2] = u64::MAX;
    cpu.x[5] = 0x1234_abcd_5678_9876;
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
        result.x[..6],
        [
            0xfedc_ba98,
            0x1234_5678_fedc_ba98,
            0,
            0x1234_0000,
            0xffff_ffff,
            0xbeef_9876
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
fn pointer_authentication_exits_before_modifying_guest_registers() {
    use bifrost_runtime::core::{
        Arm64PointerAction, Arm64PointerAuth, Arm64PointerKey, Arm64PointerModifier,
    };

    let code = [0xdac1_01f0]; // pacia x16, x15
    let (mut memory, mut thread) = memory_and_thread(&code, Protection::ReadWrite);
    let original = thread.clone();
    let expected = ExitReason::Arm64PointerAuth {
        pc: CODE,
        operation: Arm64PointerAuth {
            action: Arm64PointerAction::Sign,
            key: Arm64PointerKey::InstructionA,
            pointer_register: 16,
            modifier: Arm64PointerModifier::Register(15),
        },
    };
    let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
        .unwrap()
        .step(&mut thread)
        .unwrap();
    assert_eq!(exit, expected);
    assert_eq!(thread, original);
    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        use bifrost_runtime::raw::{RawJit, SystemJitMemory};
        let exit = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
            .unwrap()
            .step(&mut thread)
            .unwrap();
        assert_eq!(exit, expected);
        assert_eq!(thread, original);
    }
}

#[test]
fn scalar_memory_access_preserves_adjacent_bytes_and_zero_extends_loads() {
    check_memory_access([0x3900_0420, 0x3940_0422], DATA.0, 1, &[0xab]); // strb/ldrb [x1,#1]
    check_memory_access([0x381f_f020, 0x385f_f022], DATA.0 + 2, 1, &[0xab]); // sturb/ldurb [x1,#-1]
    check_memory_access([0x7900_0420, 0x7940_0422], DATA.0, 2, &[0xab, 0xef]); // strh/ldrh [x1,#2]
    check_memory_access([0x781f_f020, 0x785f_f022], DATA.0 + 2, 1, &[0xab, 0xef]); // sturh/ldurh [x1,#-1]
    check_memory_access([0x089f_fc20, 0x38bf_c022], DATA.0, 0, &[0xab]); // stlrb/ldaprb
    check_memory_access([0x489f_fc20, 0x78bf_c022], DATA.0, 0, &[0xab, 0xef]); // stlrh/ldaprh
    check_memory_access(
        [0xb900_0420, 0xb940_0422],
        DATA.0,
        4,
        &[0xab, 0xef, 0xcd, 0x89],
    ); // str/ldr [x1,#4]
    check_memory_access(
        [0xb81f_f020, 0xb85f_f022],
        DATA.0 + 2,
        1,
        &[0xab, 0xef, 0xcd, 0x89],
    ); // stur/ldur [x1,#-1]
    check_memory_access(
        [0xf81f_f020, 0xf85f_f022],
        DATA.0 + 1,
        0,
        &[0xab, 0xef, 0xcd, 0x89, 0x78, 0x56, 0x34, 0x12],
    ); // stur/ldur X registers [x1,#-1]
    check_memory_access(
        [0xb800_4c20, 0xb85f_c422],
        DATA.0,
        4,
        &[0xab, 0xef, 0xcd, 0x89],
    ); // str [x1,#4]!; ldr [x1],#-4
    check_memory_access(
        [0xf800_8c20, 0xf85f_8422],
        DATA.0 - 7,
        1,
        &[0xab, 0xef, 0xcd, 0x89, 0x78, 0x56, 0x34, 0x12],
    ); // str [x1,#8]!; ldr [x1],#-8
}

#[test]
fn scalar_arithmetic_and_logic_use_the_correct_width_and_flags() {
    const NZCV: u32 = 0xa000_0000;
    // The inputs carry nonzero high halves to catch accidental 64-bit arithmetic.
    for (word, lhs, rhs, expected, flags) in [
        (0x0b02_0020, 0xffff_ffff, 1, 0, NZCV), // add w0,w1,w2
        (0x8b02_0820, 1, 2, 0xf49f_c168_0000_0009, NZCV), // add x0,x1,x2,lsl #2
        (0x6b82_7c20, 1, 0x8000_0000, 2, 0),    // subs w0,w1,w2,asr #31
        (0x0b42_1020, 1, 0x80, 9, NZCV),        // add w0,w1,w2,lsr #4
        (0x4b02_0020, 0, 1, 0xffff_ffff, NZCV), // sub w0,w1,w2
        (0x4b22_0020, 300, 0x1ff, 45, NZCV),    // sub w0,w1,w2,uxtb
        (0xab22_8820, 1, 0xff, 0xabcd_1233_ffff_fffd, NZCV), // adds x0,x1,w2,sxtb #2
        (0x8b22_4fff, 0, 2, u64::MAX, NZCV),    // add sp,sp,w2,uxtw #3
        (0x2b02_0020, 0xffff_ffff, 1, 0, 0x6000_0000), // adds, zero and carry
        (0x2b02_0020, 0x7fff_ffff, 1, 0x8000_0000, 0x9000_0000), // adds, overflow
        (0x6b02_0020, 0, 1, 0xffff_ffff, 0x8000_0000), // subs, borrow
        (0x6b02_0020, 0x8000_0000, 1, 0x7fff_ffff, 0x3000_0000), // subs, overflow and no borrow
        (0x1140_0420, 0xab, 0, 0x10ab, NZCV),   // add w0,w1,#1,lsl #12
        (0x7100_043f, 1, 0, u64::MAX, 0x6000_0000), // cmp w1,#1
        (0x1100_043f, 0xffff_ffff, 0, u64::MAX, NZCV), // add wsp,w1,#1
        (0x3200_f3e0, 0, 0, 0x5555_5555, NZCV), // orr w0,wzr,#0x55555555
        (0x1208_9c20, 0x1234_56ab, 0, 0x1200_5600, NZCV), // and w0,w1,#0xff00ff00
        (0xd241_0420, 1, 0, 0x2bcd_1234_0000_0000, NZCV), // eor x0,x1,#0x8000000000000001
        (0x7201_0020, 0x8000_0000, 0, 0x8000_0000, 0x8000_0000), // ands w0,w1,#0x80000000, negative
        (0x7201_0020, 0x7fff_ffff, 0, 0, 0x4000_0000), // ands w0,w1,#0x80000000, zero
        (0xf241_043f, 0, 0, u64::MAX, 0x8000_0000), // tst x1,#0x8000000000000001
        (0x3200_e03f, 2, 0, u64::MAX, NZCV),    // orr wsp,w1,#0x11111111
        (0x1b02_7c20, 0xffff_ffff, 2, 0xffff_fffe, NZCV), // mul w0,w1,w2
        (0x1b02_0c20, 2, 3, 13, NZCV),          // madd w0,w1,w2,w3
        (0x1b02_8c20, 2, 4, 0xffff_ffff, NZCV), // msub w0,w1,w2,w3
        (0x1b02_fc20, 2, 3, 0xffff_fffa, NZCV), // mneg w0,w1,w2
        (0x9b02_0c20, 2, 3, 0x27d0_8e36_0000_000d, NZCV), // madd x0,x1,x2,x3
        (0x9b02_8c20, 2, 3, 0xd82f_71ca_0000_0001, NZCV), // msub x0,x1,x2,x3
        (0x9bc2_7c20, u64::MAX, 2, 0x1234_abcd_0000_0001, NZCV), // umulh x0,x1,x2
        (0x9b42_7c20, u64::MAX, 2, u64::MAX, NZCV), // smulh x0,x1,x2
        (0x9ba2_0c20, u32::MAX.into(), 2, 0x2_0000_0005, NZCV), // umaddl x0,w1,w2,x3
        (0x9b22_8c20, u32::MAX.into(), 2, 9, NZCV), // smsubl x0,w1,w2,x3
        (0xaa02_8020, 3, 1, 0xabcd_1235_0000_0003, NZCV), // orr x0,x1,x2,lsl #32
        (0x2a82_07e0, 0, 0x8000_0000, 0xc000_0000, NZCV), // orr w0,wzr,w2,asr #1
        (0x4ac2_0420, 2, 1, 0x8000_0002, NZCV), // eor w0,w1,w2,ror #1
        (0x0a42_1020, 0xff, 0xdeb0, 0xeb, NZCV), // and w0,w1,w2,lsr #4
        (0x6a22_0420, 2, 1, 0, 0x4000_0000),    // bics w0,w1,w2,lsl #1
        (0xd343_fc20, 8, 0, 0x1579_a246_8000_0001, NZCV), // lsr x0,x1,#3
        (0xdac0_0020, 1, 0, 0x8000_0000_2c48_b3d5, NZCV), // rbit x0,x1
        (0x5ac0_0020, 0x8000_000f, 0, 0xf000_0001, NZCV), // rbit w0,w1
        (0x5ac0_0820, 0x1020_3040, 0, 0x4030_2010, NZCV), // rev w0,w1
        (0x5ac0_0420, 0x1020_3040, 0, 0x2010_4030, NZCV), // rev16 w0,w1
        (0xdac0_0420, 0x1020_3040, 0, 0xcdab_3412_2010_4030, NZCV), // rev16 x0,x1
        (0xdac0_0820, 0x1020_3040, 0, 0x3412_cdab_4030_2010, NZCV), // rev32 x0,x1
        (0xdac0_0c20, 0x1020_3040, 0, 0x4030_2010_3412_cdab, NZCV), // rev x0,x1
        (0xdac0_1020, 0, 0, 0, NZCV),           // clz x0,x1
        (0x5ac0_1020, 0, 0, 32, NZCV),          // clz w0,w1 with zero low word
        (0x5ac0_1420, 0xff00_0000, 0, 7, NZCV), // cls w0,w1 with negative low word
        (0x5ac0_1420, 0x0400_0000, 0, 4, NZCV), // cls w0,w1 with positive low word
        (0x9ac2_2020, 1, 65, 0x579a_2468_0000_0002, NZCV), // lsl x0,x1,x2 (count wraps to 1)
        (0x1ac2_2420, 0x8000_0000, 33, 0x4000_0000, NZCV), // lsr w0,w1,w2
        (0x1ac2_2820, 0x8000_0000, 33, 0xc000_0000, NZCV), // asr w0,w1,w2
        (0x1ac2_2c20, 1, 33, 0x8000_0000, NZCV), // ror w0,w1,w2
        (0x1a82_0020, 5, 7, 7, NZCV),           // csel w0,w1,w2,eq
        (0x1a82_1020, 5, 7, 5, NZCV),           // csel w0,w1,w2,ne
        (0x1a82_0420, 5, 0xffff_ffff, 0, NZCV), // csinc w0,w1,w2,eq
        (0xda82_0020, 5, 7, 0xedcb_5432_ffff_fff8, NZCV), // csinv x0,x1,x2,eq
        (0x5a82_0420, 5, 2, 0xffff_fffe, NZCV), // csneg w0,w1,w2,eq
        (0xfa51_1824, 1, 0, u64::MAX, NZCV),    // ccmp x1,#17,#4,ne
        (0x7a42_0023, 1, 1, u64::MAX, 0x3000_0000), // ccmp w1,w2,#3,eq (fallback)
        (0x3a41_1825, 0xffff_ffff, 0, u64::MAX, 0x6000_0000), // ccmn w1,#1,#5,ne
        (0xba42_0028, 0, 0, u64::MAX, 0x8000_0000), // ccmn x1,x2,#8,eq (fallback)
        (0x5318_5c20, 0x1234_5678, 0, 0x3456_7800, NZCV), // lsl w0,w1,#8
        (0x1304_7c20, 0x8123_4567, 0, 0xf812_3456, NZCV), // asr w0,w1,#4
        (0x9340_1c20, 0x80, 0, u64::MAX - 127, NZCV), // sxtb x0,w1
        (0x1318_1c20, 0x80, 0, 0xffff_8000, NZCV), // sbfiz w0,w1,#8,#8
        (0x5308_3c20, 0x1234, 0, 0x12, NZCV),   // ubfx w0,w1,#8,#8
        (0x3318_1c20, 0x1234, 0, 0xffff_34ff, NZCV), // bfi w0,w1,#8,#8
        (0x3308_3c20, 0x1234, 0, 0xffff_ff12, NZCV), // bfxil w0,w1,#8,#8
    ] {
        let (mut memory, mut thread) = memory_and_thread(&[word], Protection::ReadWrite);
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.x[0] = u64::MAX;
        cpu.x[1] = 0xabcd_1234_0000_0000 | lhs;
        cpu.x[2] = 0x1234_abcd_0000_0000 | rhs;
        cpu.x[3] = 7;
        cpu.nzcv = NZCV;
        let original = thread.clone();
        let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .step(&mut thread)
            .unwrap();
        assert_eq!(exit, ExitReason::Dispatch(Addr(CODE.0 + 4)));
        assert_eq!(state(&thread).x[0], expected, "instruction {word:#x}");
        assert_eq!(state(&thread).nzcv, flags, "instruction {word:#x}");
        let sp = match word {
            0x8b22_4fff => Addr(state(&original).sp.0 + 16),
            0x1100_043f => Addr(0),
            0x3200_e03f => Addr(0x1111_1113),
            _ => state(&original).sp,
        };
        assert_eq!(state(&thread).sp, sp);

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
                .step(&mut jitted)
                .unwrap();
            assert_eq!(actual, exit);
            assert_eq!(jitted, thread);
        }
    }
}

#[test]
fn register_offsets_extend_and_scale_before_the_memory_access() {
    check_memory_access(
        [0xb823_5820, 0xb863_5822],
        DATA.0,
        4,
        &[0xab, 0xef, 0xcd, 0x89],
    );
    check_memory_access([0x3824_c820, 0x3864_c822], DATA.0 + 2, 1, &[0xab]);
    check_memory_access(
        [0xf825_6820, 0xf865_6822],
        DATA.0,
        1,
        &[0xab, 0xef, 0xcd, 0x89, 0x78, 0x56, 0x34, 0x12],
    );
    check_memory_access([0x7825_7820, 0x7865_7822], DATA.0, 2, &[0xab, 0xef]);
    check_memory_access_result(
        [0x7825_7820, 0x78e5_7822],
        DATA.0,
        2,
        &[0xab, 0xef],
        0xffff_efab,
    );
    check_memory_access_result(
        [0x3824_c820, 0x38a4_c822],
        DATA.0 + 2,
        1,
        &[0xab],
        0xffff_ffff_ffff_ffab,
    );
    check_memory_access_result(
        [0xb823_5820, 0xb8a3_5822],
        DATA.0,
        4,
        &[0xab, 0xef, 0xcd, 0x89],
        0xffff_ffff_89cd_efab,
    );
}

fn check_memory_access(code: [u32; 2], base: u64, offset: usize, expected_bytes: &[u8]) {
    let mut loaded = [0; 8];
    loaded[..expected_bytes.len()].copy_from_slice(expected_bytes);
    check_memory_access_result(
        code,
        base,
        offset,
        expected_bytes,
        u64::from_le_bytes(loaded),
    );
}

fn check_memory_access_result(
    code: [u32; 2],
    base: u64,
    offset: usize,
    expected_bytes: &[u8],
    loaded: u64,
) {
    let (mut memory, mut thread) = memory_and_thread(&code, Protection::ReadWrite);
    let initial = [1, 2, 3, 4, 5, 6, 7, 8, 9];
    let mut expected = initial;
    expected[offset..offset + expected_bytes.len()].copy_from_slice(expected_bytes);
    memory.write(DATA, &initial).unwrap();
    let CpuState::AArch64(cpu) = &mut thread.cpu else {
        unreachable!()
    };
    cpu.x[0] = 0x1234_5678_89cd_efab;
    cpu.x[1] = base;
    cpu.x[2] = u64::MAX;
    cpu.x[3] = 0xfedc_ba98_0000_0001;
    cpu.x[4] = 0x1234_abcd_ffff_ffff;
    cpu.x[5] = 1;
    let original = thread.clone();
    let budget = ExecutionBudget { instructions: 2 };
    let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
        .unwrap()
        .run(&mut thread, budget)
        .unwrap();
    assert_eq!(exit, ExitReason::BudgetExhausted);
    let mut bytes = [0; 9];
    memory.read(DATA, &mut bytes).unwrap();
    assert_eq!(bytes, expected);
    assert_eq!(state(&thread).x[2], loaded);
    assert_eq!(state(&thread).x[1], base);
    assert_eq!(state(&thread).x[0], state(&original).x[0]);

    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        use bifrost_runtime::raw::{RawJit, SystemJitMemory};
        memory.write(DATA, &initial).unwrap();
        let mut jitted = original;
        let actual = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
            .unwrap()
            .run(&mut jitted, budget)
            .unwrap();
        assert_eq!(actual, exit);
        assert_eq!(jitted, thread);
        memory.read(DATA, &mut bytes).unwrap();
        assert_eq!(bytes, expected);
    }
}

#[test]
fn indexed_memory_faults_leave_the_base_and_destination_intact() {
    for (word, base) in [
        (0xf800_8c20, DATA.0 + 4096 - 8),
        (0xf85f_8422, DATA.0 + 4096),
        (0x3840_1422, DATA.0 + 4096), // ldrb w2, [x1], #1
        (0x7800_2c20, DATA.0 + 4094), // strh w0, [x1, #2]!
    ] {
        let (mut memory, mut thread) = memory_and_thread(&[word], Protection::ReadWrite);
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.x[1] = base;
        cpu.x[2] = u64::MAX;
        let original = thread.clone();
        let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .step(&mut thread)
            .unwrap();
        assert!(
            matches!(exit, ExitReason::Fault(fault) if fault.class == FaultClass::Unmapped && fault.pc == CODE)
        );
        assert_eq!(thread.cpu, original.cpu);
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
                .step(&mut jitted)
                .unwrap();
            assert_eq!(actual, exit);
            assert_eq!(jitted, thread);
        }
    }
}

#[test]
fn integer_division_handles_zero_and_signed_overflow_without_trapping() {
    for (word, lhs, rhs, expected) in [
        (0x1ac2_0820, 0xabcd_1234_0000_000f, 4, 3),
        (0x1ac2_0c20, u64::MAX - 14, 4, u64::from(u32::MAX - 2)),
        (0x1ac2_0c20, 0x8000_0000, u64::MAX, 0x8000_0000),
        (0x9ac2_0820, u64::MAX, 0, 0),
        (0x9ac2_0c20, u64::MAX, 0, 0),
        (0x9ac2_0c20, 1 << 63, u64::MAX, 1 << 63),
    ] {
        let (mut memory, mut thread) = memory_and_thread(&[word], Protection::ReadWrite);
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.x[1] = lhs;
        cpu.x[2] = rhs;
        cpu.nzcv = 0xa000_0000;
        let original = thread.clone();
        let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .step(&mut thread)
            .unwrap();
        assert_eq!(exit, ExitReason::Dispatch(Addr(CODE.0 + 4)));
        assert_eq!(state(&thread).x[0], expected);
        assert_eq!(state(&thread).nzcv, 0xa000_0000);
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
                .step(&mut jitted)
                .unwrap();
            assert_eq!(actual, exit);
            assert_eq!(jitted, thread);
        }
    }
}

#[test]
fn thread_pointer_instructions_keep_the_read_only_pointer_separate() {
    let (mut memory, mut thread) = memory_and_thread(
        &[0xd51b_d041, 0xd53b_d042, 0xd53b_d063, 0xd51b_d061],
        Protection::ReadWrite,
    );
    let CpuState::AArch64(cpu) = &mut thread.cpu else {
        unreachable!()
    };
    cpu.tpidrro_el0 = Addr(0x1234_5678);
    let original = thread.clone();
    let budget = ExecutionBudget { instructions: 4 };
    let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
        .unwrap()
        .run(&mut thread, budget)
        .unwrap();
    assert!(
        matches!(exit, ExitReason::Fault(fault) if fault.class == FaultClass::IllegalInstruction && fault.pc == Addr(CODE.0 + 12))
    );
    assert_eq!(state(&thread).x[2..4], [DATA.0, 0x1234_5678]);
    assert_eq!(state(&thread).tpidr_el0, DATA);
    assert_eq!(state(&thread).tpidrro_el0, Addr(0x1234_5678));
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
fn compare_exchange_returns_the_old_value_and_faults_before_writeback() {
    for (expected, alignment, protection, changed) in [
        (7, 0, Protection::ReadWrite, true),
        (9, 0, Protection::ReadWrite, false),
        (7, 0, Protection::Read, false),
        (7, 1, Protection::ReadWrite, false),
    ] {
        let (mut memory, mut thread) = memory_and_thread(&[0x88e1_7c02], Protection::ReadWrite);
        memory.write(DATA, &7u32.to_le_bytes()).unwrap();
        memory.protect(DATA, 4096, protection).unwrap();
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.x[0] = DATA.0 + alignment;
        cpu.x[1] = (1 << 40) | expected;
        cpu.x[2] = 42;
        let original = thread.clone();
        let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .step(&mut thread)
            .unwrap();
        let check = |exit, actual: &GuestThread, memory: &AddressSpace| {
            let mut bytes = [0; 4];
            memory.read(DATA, &mut bytes).unwrap();
            assert_eq!(u32::from_le_bytes(bytes), if changed { 42 } else { 7 });
            let fault = match (alignment, protection) {
                (1, _) => Some(FaultClass::Alignment),
                (_, Protection::Read) => Some(FaultClass::Protection),
                _ => None,
            };
            if let Some(class) = fault {
                assert!(
                    matches!(exit, ExitReason::Fault(fault) if fault.class == class && fault.pc == CODE)
                );
                assert_eq!(actual.cpu, original.cpu);
            } else {
                assert_eq!(exit, ExitReason::Dispatch(Addr(CODE.0 + 4)));
                assert_eq!(state(actual).x[1], 7);
            }
        };
        check(exit, &thread, &memory);
        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            use bifrost_runtime::raw::{RawJit, SystemJitMemory};
            if changed {
                memory.write(DATA, &7u32.to_le_bytes()).unwrap();
            }
            let mut jitted = original.clone();
            let actual = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
                .unwrap()
                .step(&mut jitted)
                .unwrap();
            check(actual, &jitted, &memory);
            assert_eq!(actual, exit);
            assert_eq!(jitted, thread);
        }
    }
}

#[test]
fn atomic_add_returns_the_old_value_and_wraps_at_the_access_width() {
    check_atomic_update(0xb8e1_0002, u64::from(u32::MAX), 1);
}

#[test]
fn atomic_clear_returns_the_old_value_and_only_clears_selected_bits() {
    check_atomic_update(0xb8e1_1002, u64::from(u32::MAX), u64::from(u32::MAX - 2));
}

#[test]
fn atomic_set_returns_the_old_value_and_only_sets_selected_bits() {
    check_atomic_update(0xb8e1_3002, 0x81, 0x83);
}

#[test]
fn atomic_swap_returns_the_old_value_and_preserves_adjacent_bytes() {
    check_atomic_update(0x38a1_8002, 0xab, 2); // swpab w1, w2, [x0]
    check_atomic_update(0xf8e1_8002, 0x1234_5678_9abc_def0, 2); // swpal x1, x2, [x0]
}

fn check_atomic_update(word: u32, initial_word: u64, expected_word: u64) {
    let (mut memory, mut thread) = memory_and_thread(&[word], Protection::ReadWrite);
    let width = 1 << (word >> 30);
    let mut initial = [0x66; 9];
    initial[..width].copy_from_slice(&initial_word.to_le_bytes()[..width]);
    memory.write(DATA, &initial).unwrap();
    let CpuState::AArch64(cpu) = &mut thread.cpu else {
        unreachable!()
    };
    cpu.x[0] = DATA.0;
    cpu.x[1] = 2;
    cpu.x[2] = u64::MAX;
    let original = thread.clone();
    let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
        .unwrap()
        .step(&mut thread)
        .unwrap();
    let check = |thread: &GuestThread, memory: &AddressSpace| {
        assert_eq!(state(thread).x[1..3], [2, initial_word]);
        let mut bytes = [0; 9];
        memory.read(DATA, &mut bytes).unwrap();
        assert_eq!(bytes[..width], expected_word.to_le_bytes()[..width]);
        assert_eq!(bytes[width..], initial[width..]);
    };
    assert_eq!(exit, ExitReason::Dispatch(Addr(CODE.0 + 4)));
    check(&thread, &memory);
    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        use bifrost_runtime::raw::{RawJit, SystemJitMemory};
        memory.write(DATA, &initial).unwrap();
        let mut jitted = original;
        let actual = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
            .unwrap()
            .step(&mut jitted)
            .unwrap();
        assert_eq!(actual, exit);
        assert_eq!(jitted, thread);
        check(&jitted, &memory);
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

#[test]
fn test_branches_use_the_selected_register_width_and_bit() {
    for (word, value, displacement) in [
        (0x3400_0041, 1 << 32, 8), // cbz w1, #8
        (0x3500_0041, 1 << 32, 4), // cbnz w1, #8 (fall through)
        (0x3500_0041, (1 << 32) | 1, 8),
        (0xb6ff_ffe1, 1 << 62, -4), // tbz x1, #63, #-4
        (0xb7f8_0041, 1 << 63, 8),  // tbnz x1, #63, #8
        (0x3600_0041, 1, 4),        // tbz w1, #0, #8 (fall through)
    ] {
        let (mut memory, mut thread) = memory_and_thread(&[word], Protection::ReadWrite);
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.x[1] = value;
        cpu.nzcv = 0xf000_0000;
        let mut expected = thread.clone();
        let pc = Addr(CODE.0.wrapping_add_signed(displacement));
        expected.cpu.set_pc(pc);
        let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .step(&mut thread)
            .unwrap();
        assert_eq!(exit, ExitReason::Dispatch(pc));
        assert_eq!(thread, expected);
        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            use bifrost_runtime::raw::{RawJit, SystemJitMemory};
            let mut jitted = expected.clone();
            jitted.cpu.set_pc(CODE);
            let actual = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
                .unwrap()
                .step(&mut jitted)
                .unwrap();
            assert_eq!(actual, exit);
            assert_eq!(jitted, expected);
        }
    }
}

#[test]
fn register_pairs_transfer_contiguous_values_and_guard_the_whole_span() {
    check_register_pair([0x293f_8820, 0x297f_9023], 4, false);
    check_register_pair([0xa93f_8820, 0xa97f_9023], 8, false);
    check_register_pair([0x29bf_8820, 0x28c0_9023], 4, true);
    check_register_pair([0xa9bf_8820, 0xa8c0_9023], 8, true);
}

#[test]
fn indirect_branch_with_link_uses_the_original_destination() {
    for (word, destination, link) in [
        (0xd61f_0120, DATA, false),            // br x9
        (0xd63f_03c0, Addr(DATA.0 + 4), true), // blr x30
        (0xd61f_03e0, Addr(0), false),         // br xzr
    ] {
        let (mut memory, mut thread) = memory_and_thread(&[word], Protection::ReadWrite);
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.x[9] = DATA.0;
        cpu.x[30] = DATA.0 + 4;
        let original = thread.clone();
        let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .step(&mut thread)
            .unwrap();
        assert_eq!(exit, ExitReason::Dispatch(destination));
        assert_eq!(
            state(&thread).x[30],
            if link {
                CODE.0 + 4
            } else {
                state(&original).x[30]
            }
        );
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
                .step(&mut jitted)
                .unwrap();
            assert_eq!(actual, exit);
            assert_eq!(jitted, thread);
        }
    }
}

#[test]
fn stack_relative_memory_checks_sp_alignment_before_access() {
    for (code, sp, expected_access) in [
        (&[0xa901_0be0, 0xa941_13e3][..], DATA, None), // stp/ldp [sp,#16]
        (
            &[0xa901_0be0][..],
            Addr(DATA.0 + 8),
            Some(AccessKind::Write),
        ),
        (&[0xf940_07e3][..], Addr(DATA.0 + 8), Some(AccessKind::Read)), // ldr [sp,#8]
    ] {
        let (mut memory, mut thread) = memory_and_thread(code, Protection::ReadWrite);
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.sp = sp;
        cpu.x[0] = 42;
        cpu.x[2] = 43;
        let original = thread.clone();
        let budget = ExecutionBudget {
            instructions: code.len() as u64,
        };
        let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .run(&mut thread, budget)
            .unwrap();
        if let Some(access) = expected_access {
            let ExitReason::Fault(fault) = exit else {
                panic!("expected stack-pointer alignment fault")
            };
            assert_eq!(fault.class, FaultClass::Alignment);
            assert_eq!(fault.memory_access, Some((sp, access)));
            assert_eq!(thread.cpu, original.cpu);
        } else {
            assert_eq!(exit, ExitReason::BudgetExhausted);
            assert_eq!(state(&thread).x[3..5], [42, 43]);
        }
        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            use bifrost_runtime::raw::{RawJit, SystemJitMemory};
            memory.write(Addr(DATA.0 + 16), &[0; 16]).unwrap();
            let mut jitted = original;
            let actual = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
                .unwrap()
                .run(&mut jitted, budget)
                .unwrap();
            assert_eq!(actual, exit);
            assert_eq!(jitted, thread);
        }
    }
}

fn check_register_pair(code: [u32; 2], width: usize, writeback: bool) {
    let first: u64 = 0xdead_beef_1122_3344;
    let second: u64 = 0xdead_beef_aabb_ccdd;
    for fault in [None, Some(AccessKind::Write), Some(AccessKind::Read)] {
        let code = if fault == Some(AccessKind::Read) {
            &code[1..]
        } else {
            &code[..]
        };
        let (mut memory, mut thread) = memory_and_thread(code, Protection::ReadWrite);
        let address = if fault.is_some() {
            Addr(DATA.0 + 4096 - width as u64)
        } else {
            DATA
        };
        let size = if fault.is_some() { width } else { width * 2 };
        let before = &[0x77; 16][..size];
        memory.write(address, before).unwrap();
        let CpuState::AArch64(cpu) = &mut thread.cpu else {
            unreachable!()
        };
        cpu.x[0] = first;
        cpu.x[1] = if writeback && fault == Some(AccessKind::Read) {
            address.0
        } else {
            address.0 + width as u64
        };
        cpu.x[2] = second;
        cpu.x[3] = u64::MAX;
        cpu.x[4] = u64::MAX;
        let original = thread.clone();
        let budget = ExecutionBudget { instructions: 2 };
        let exit = RawInterpreter::new(&target(), &mut memory, &mut ())
            .unwrap()
            .run(&mut thread, budget)
            .unwrap();
        let mut bytes = [0; 16];
        memory.read(address, &mut bytes[..size]).unwrap();
        if let Some(access) = fault {
            let ExitReason::Fault(fault) = exit else {
                panic!("pair must fault at the page boundary")
            };
            assert_eq!(fault.pc, CODE);
            assert_eq!(fault.class, FaultClass::Unmapped);
            assert_eq!(fault.memory_access.map(|(_, kind)| kind), Some(access));
            assert_eq!(&bytes[..size], before);
            assert_eq!(thread.cpu, original.cpu);
        } else {
            assert_eq!(exit, ExitReason::BudgetExhausted);
            let mask = if width == 4 {
                u64::from(u32::MAX)
            } else {
                u64::MAX
            };
            assert_eq!(state(&thread).x[3..5], [first & mask, second & mask]);
            assert_eq!(state(&thread).x[1], address.0 + width as u64);
            assert_eq!(bytes[..width], first.to_le_bytes()[..width]);
            assert_eq!(bytes[width..size], second.to_le_bytes()[..width]);
        }

        #[cfg(all(
            feature = "jit-cranelift",
            any(target_arch = "x86_64", target_arch = "aarch64"),
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            use bifrost_runtime::raw::{RawJit, SystemJitMemory};
            memory.write(address, before).unwrap();
            let mut jitted = original;
            let actual = RawJit::new(&target(), &mut memory, &mut (), SystemJitMemory)
                .unwrap()
                .run(&mut jitted, budget)
                .unwrap();
            assert_eq!(actual, exit);
            assert_eq!(jitted, thread);
            let mut jitted_bytes = [0; 16];
            memory.read(address, &mut jitted_bytes[..size]).unwrap();
            assert_eq!(jitted_bytes, bytes);
        }
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
