#![cfg(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "windows")
))]

use bifrost_runtime::{
    core::{
        AbiId, Arm64State, CpuState, GuestAddress as Addr, GuestIsa, GuestOs, GuestTarget,
        GuestThread, GuestThreadId,
    },
    memory::{AddressSpace, GuestMemory, Protection},
    raw::{ExecutionBudget, Executor, ExitReason, RawInterpreter, RawJit, SystemJitMemory},
};
use bifrost_testkit::arm64::bytes;

#[cfg(target_arch = "aarch64")]
#[allow(unsafe_code)]
mod hardware_arm64;

const ENTRY: Addr = Addr(0x1000);

fn code(words: &[u32]) -> AddressSpace {
    let mut memory = AddressSpace::new(4096, 1).unwrap();
    memory.map(ENTRY, 4096, Protection::ReadWrite).unwrap();
    memory.write(ENTRY, &bytes(words)).unwrap();
    memory
        .protect(ENTRY, 4096, Protection::ReadExecute)
        .unwrap();
    memory
}

fn test_guest(words: &[u32], run: impl FnOnce(&mut dyn FnMut(Arm64State, u64) -> Arm64State)) {
    let target = GuestTarget::new(
        GuestIsa::AArch64,
        GuestOs::None,
        AbiId::Bare,
        Default::default(),
    )
    .unwrap();
    let (mut interpreted, mut compiled) = (code(words), code(words));
    let (mut interp_trace, mut jit_trace) = ((), ());
    let mut interpreter =
        RawInterpreter::new(&target, &mut interpreted, &mut interp_trace).unwrap();
    let mut jit = RawJit::new(&target, &mut compiled, &mut jit_trace, SystemJitMemory).unwrap();
    run(&mut |state, instructions| {
        let initial = GuestThread {
            id: GuestThreadId(1),
            cpu: CpuState::AArch64(Box::new(state)),
            pending_fault: None,
        };
        let (mut reference, mut actual) = (initial.clone(), initial);
        let budget = ExecutionBudget { instructions };
        assert_eq!(
            interpreter.run(&mut reference, budget).unwrap(),
            ExitReason::BudgetExhausted
        );
        assert_eq!(
            jit.run(&mut actual, budget).unwrap(),
            ExitReason::BudgetExhausted
        );
        assert_eq!(reference, actual);
        let CpuState::AArch64(cpu) = actual.cpu else {
            panic!("expected ARM64 guest");
        };
        *cpu
    });
}

fn reference_arithmetic(lhs: u64, rhs: u64, sub: bool) -> (u64, u32) {
    let (result, carry, overflow) = if sub {
        let (result, borrow) = lhs.overflowing_sub(rhs);
        (result, !borrow, (lhs as i64).overflowing_sub(rhs as i64).1)
    } else {
        let (result, carry) = lhs.overflowing_add(rhs);
        (result, carry, (lhs as i64).overflowing_add(rhs as i64).1)
    };
    let negative = u32::from((result as i64).is_negative()) << 31;
    let zero = u32::from(result == 0) << 30;
    let flags = negative | zero | (u32::from(carry) << 29) | (u32::from(overflow) << 28);
    (result, flags)
}

const EDGE: [u64; 10] = [
    0,
    1,
    2,
    0xfff,
    0x1000,
    i64::MAX as u64,
    1 << 63,
    (1 << 63) + 1,
    u64::MAX - 1,
    u64::MAX,
];

fn pairs() -> impl Iterator<Item = (u64, u64)> {
    let boundary = EDGE
        .into_iter()
        .flat_map(|lhs| EDGE.into_iter().map(move |rhs| (lhs, rhs)));
    let mut seed = 0x4c96_5c61_aef7_1937_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    boundary.chain((0..128).map(move |_| (next(), next())))
}

#[test]
fn arithmetic_matches_independent_flags_and_native_a64_when_available() {
    // ADDS/SUBS X0,X1,X2; ADDS/SUBS X0,X1,#4095,LSL #12.
    let words = [0xab02_0020, 0xeb02_0020, 0xb17f_fc20, 0xf17f_fc20];
    test_guest(&words, |step| {
        for (lhs, rhs) in pairs() {
            for index in 0..words.len() {
                let sub = index % 2 != 0;
                let actual_rhs = if index >= 2 { 0xfff000 } else { rhs };
                let expected = reference_arithmetic(lhs, actual_rhs, sub);
                #[cfg(target_arch = "aarch64")]
                assert_eq!(hardware_arm64::arithmetic(lhs, actual_rhs, sub), expected);
                let mut state = Arm64State {
                    pc: Addr(ENTRY.0 + 4 * index as u64),
                    nzcv: 0xf000_0000,
                    ..Default::default()
                };
                state.x[1] = lhs;
                state.x[2] = rhs;
                let output = step(state, 1);
                assert_eq!(
                    (output.x[0], output.nzcv),
                    expected,
                    "form {index} lhs={lhs:#x} rhs={actual_rhs:#x}"
                );
            }
        }
    });
}

fn expected_branch(condition: usize, flags: u32) -> bool {
    let [n, z, c, v] = [31, 30, 29, 28].map(|bit| flags & (1 << bit) != 0);
    [
        z,
        !z,
        c,
        !c,
        n,
        !n,
        v,
        !v,
        c && !z,
        !c || z,
        n == v,
        n != v,
        !z && n == v,
        z || n != v,
        true,
        true,
    ][condition]
}

#[test]
fn branches_match_all_nzcv_combinations_and_native_a64_when_available() {
    let words = std::array::from_fn::<_, 16, _>(|index| 0x5400_0040 | index as u32);
    test_guest(&words, |step| {
        for condition in 0..16 {
            for flags in (0..16).map(|v| v << 28) {
                let expected = expected_branch(condition, flags);
                #[cfg(target_arch = "aarch64")]
                assert_eq!(hardware_arm64::branch(condition, flags), expected);
                let pc = Addr(ENTRY.0 + 4 * condition as u64);
                let state = Arm64State {
                    pc,
                    nzcv: flags,
                    ..Default::default()
                };
                let output = step(state, 1);
                assert_eq!(output.pc, Addr(pc.0 + if expected { 8 } else { 4 }));
                assert_eq!(output.nzcv, flags);
            }
        }
    });
}

#[test]
fn register_31_sp_zero_and_move_wide_are_independent() {
    let words = [
        0x9280_0004, // movn x4,#0
        0xf2a2_4684, // movk x4,#0x1234,lsl #16
        0xaa04_03e5, // mov x5,x4
        0xd280_003f, // movz xzr,#1
        0x9140_07ff, // add sp,sp,#1,lsl #12
        0xd100_43ff, // sub sp,sp,#16
        0x9100_03e7, // add x7,sp,#0
        0xb400_005f, // cbz xzr,+8
        0x0000_0000, // udf
        0xb500_005f, // cbnz xzr,+8
        0xb500_0045, // cbnz x5,+8
        0x0000_0000, // udf
        0xd503_201f, // nop
    ];
    test_guest(&words, |run| {
        let state = Arm64State {
            pc: ENTRY,
            sp: Addr(0x4000),
            nzcv: 0x9000_0000,
            ..Default::default()
        };
        let output = run(state, 11);
        assert_eq!(output.x[4], 0xffff_ffff_1234_ffff);
        assert_eq!(output.x[5], output.x[4]);
        assert_eq!(output.sp, Addr(0x4ff0));
        assert_eq!(output.x[7], 0x4ff0);
        assert_eq!(output.nzcv, 0x9000_0000);
        assert_eq!(output.pc, Addr(ENTRY.0 + words.len() as u64 * 4));
    });
}
