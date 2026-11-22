#![cfg(any(
    target_arch = "x86_64",
    all(target_arch = "aarch64", target_os = "linux")
))]

#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
#[path = "hardware.rs"]
mod reference;

#[cfg(target_arch = "aarch64")]
#[path = "portable_x86.rs"]
mod reference;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use bifrost_runtime::x86::{SystemJitMemory, X86Jit};
use bifrost_runtime::{
    core::{CpuState, GuestAddress as Addr, GuestThread, GuestThreadId, X86State},
    memory::{AddressSpace, GuestMemory, GuestMemoryRead, Protection},
    x86::{ExecutionBudget, Executor, ExitReason, X86Interpreter},
};
use bifrost_testkit::x86::{ARITHMETIC_FLAGS, CALL_AND_RETURN, MOVE_AND_ADD};

#[derive(Clone, Copy, Debug)]
enum Arithmetic {
    Add,
    Sub,
    Cmp,
}

#[derive(Clone, Copy, Debug)]
enum Branch {
    Equal,
    NotEqual,
    Below,
    AboveOrEqual,
    Less,
    GreaterOrEqual,
}

const ENTRY: Addr = Addr(0x1000);
const DATA: Addr = Addr(0x2000);

fn address_space(code: &[u8]) -> AddressSpace {
    let mut memory = AddressSpace::new(4096, 2).unwrap();
    memory.map(ENTRY, 4096, Protection::ReadWrite).unwrap();
    memory.write(ENTRY, code).unwrap();
    memory
        .protect(ENTRY, 4096, Protection::ReadExecute)
        .unwrap();
    memory.map(DATA, 4096, Protection::ReadWrite).unwrap();
    memory
}

fn interpret(code: &[u8], count: u64, registers: &[(usize, u64)]) -> (X86State, AddressSpace) {
    let mut memory = address_space(code);
    let mut cpu = X86State {
        rip: ENTRY,
        ..X86State::default()
    };
    for &(index, value) in registers {
        cpu.gpr[index] = value;
    }
    let mut thread = GuestThread {
        id: GuestThreadId(1),
        cpu: CpuState::X86_64(Box::new(cpu)),
        pending_fault: None,
    };
    let budget = ExecutionBudget {
        instructions: count,
    };
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let mut jitted = thread.clone();
    let exit = X86Interpreter::new(&mut memory, &mut ())
        .run(&mut thread, budget)
        .unwrap();
    assert_eq!(exit, ExitReason::BudgetExhausted);
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let mut native_memory = address_space(code);
        let actual = X86Jit::new(&mut native_memory, &mut (), SystemJitMemory)
            .unwrap()
            .run(&mut jitted, budget)
            .unwrap();
        assert_eq!(actual, exit);
        assert_eq!(jitted, thread, "JIT CPU state for {code:02x?}");
        let (mut native_data, mut interp_data) = ([0; 4096], [0; 4096]);
        native_memory.read(DATA, &mut native_data).unwrap();
        memory.read(DATA, &mut interp_data).unwrap();
        assert_eq!(native_data, interp_data, "JIT memory for {code:02x?}");
    }
    let CpuState::X86_64(cpu) = thread.cpu else {
        unreachable!("the fixture uses x86");
    };
    (*cpu, memory)
}

fn read_qword(memory: &AddressSpace, address: Addr) -> [u8; 8] {
    let mut bytes = [0; 8];
    memory.read(address, &mut bytes).unwrap();
    bytes
}

const EDGE_VALUES: [u64; 14] = [
    0,
    1,
    0xf,
    0x10,
    0x7f,
    0x80,
    0xff,
    0x100,
    i64::MAX as u64,
    1 << 63,
    (1 << 63) + 1,
    u64::MAX - 1,
    u64::MAX,
    0xdead_beef_0123_4567,
];

fn random_pairs(count: usize) -> impl Iterator<Item = (u64, u64)> {
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..count).map(move |_| (next(), next()))
}

#[test]
fn arithmetic_register_and_immediate_forms_match_reference() {
    let pairs = EDGE_VALUES
        .into_iter()
        .flat_map(|lhs| EDGE_VALUES.into_iter().map(move |rhs| (lhs, rhs)));
    for (lhs, rhs) in pairs.chain(random_pairs(128)) {
        for (op, rm_reg, reg_rm, accumulator, extension) in [
            (Arithmetic::Add, 0x01, 0x03, 0x05, 0),
            (Arithmetic::Sub, 0x29, 0x2b, 0x2d, 5),
            (Arithmetic::Cmp, 0x39, 0x3b, 0x3d, 7),
        ] {
            let imm8 = rhs as i8;
            let imm32 = rhs as i32;
            let modrm = 0xc0 | (extension << 3);

            let mut imm32_encoding = vec![0x48, 0x81, modrm];
            imm32_encoding.extend(imm32.to_le_bytes());
            let mut accumulator_encoding = vec![0x48, accumulator];
            accumulator_encoding.extend(imm32.to_le_bytes());
            let encodings = [
                (vec![0x48, rm_reg, 0xd8], rhs),
                (vec![0x48, reg_rm, 0xc3], rhs),
                (vec![0x48, 0x83, modrm, imm8 as u8], i64::from(imm8) as u64),
                (imm32_encoding, i64::from(imm32) as u64),
                (accumulator_encoding, i64::from(imm32) as u64),
            ];
            for (encoding, effective_rhs) in encodings {
                let (expected_rax, expected_flags) = reference::arithmetic(op, lhs, effective_rhs);
                let (guest, _) = interpret(&encoding, 1, &[(0, lhs), (3, rhs)]);
                assert_eq!(
                    (guest.gpr[0], guest.rflags & ARITHMETIC_FLAGS),
                    (expected_rax, expected_flags & ARITHMETIC_FLAGS),
                    "{op:?} bytes {encoding:02x?} lhs={lhs:#x} rhs={effective_rhs:#x}",
                );
                assert_eq!(guest.rip, Addr(ENTRY.0 + encoding.len() as u64));
                assert_eq!(guest.gpr[3], rhs);
            }
        }
    }
}

#[test]
fn short_and_near_conditionals_match_reference_branch_choices() {
    for (condition, opcode) in [
        (Branch::Equal, 0x74),
        (Branch::NotEqual, 0x75),
        (Branch::Below, 0x72),
        (Branch::AboveOrEqual, 0x73),
        (Branch::Less, 0x7c),
        (Branch::GreaterOrEqual, 0x7d),
    ] {
        let edges = [
            (0, 0),
            (0, 1),
            (1, 0),
            (u64::MAX, 0),
            (0, u64::MAX),
            (1 << 63, 1),
            (i64::MAX as u64, u64::MAX),
        ];
        for (lhs, rhs) in edges.into_iter().chain(random_pairs(32)) {
            let taken = reference::branch(condition, lhs, rhs);
            for branch in [vec![opcode, 4], vec![0x0f, opcode + 0x10, 4, 0, 0, 0]] {
                let mut code = vec![0x48, 0x39, 0xd8]; // cmp rax, rbx
                code.extend(branch);
                let (guest, _) = interpret(&code, 2, &[(0, lhs), (3, rhs)]);
                let expected = Addr(ENTRY.0 + code.len() as u64 + if taken { 4 } else { 0 });
                assert_eq!(
                    guest.rip, expected,
                    "{condition:?} lhs={lhs:#x} rhs={rhs:#x}"
                );
            }
        }
    }
}

#[test]
fn unaligned_and_indexed_memory_match_reference_values() {
    let expected = reference::move_and_add();
    let (guest, memory) = interpret(MOVE_AND_ADD, 4, &[(4, DATA.0 + 9)]);
    assert_eq!(
        (
            guest.gpr[0],
            read_qword(&memory, Addr(DATA.0 + 1)),
            guest.rflags & ARITHMETIC_FLAGS
        ),
        (expected.0, expected.1, expected.2 & ARITHMETIC_FLAGS)
    );

    // mov qword [rax + rcx*4 + 1], -2; mov rax, [rax + rcx*4 + 1]
    let code = [
        0x48, 0xc7, 0x44, 0x88, 1, 0xfe, 0xff, 0xff, 0xff, 0x48, 0x8b, 0x44, 0x88, 1,
    ];
    let (guest, memory) = interpret(&code, 2, &[(0, DATA.0), (1, 2)]);
    assert_eq!(
        (guest.gpr[0], read_qword(&memory, Addr(DATA.0 + 9))),
        reference::indexed_immediate_store()
    );
}

#[test]
fn register_moves_nop_near_jump_and_rip_relative_load_match_reference() {
    let code = [
        0x48, 0xc7, 0xc0, 0xfe, 0xff, 0xff, 0xff, // mov rax, -2
        0x48, 0x89, 0xc1, // mov rcx, rax
        0x90, // nop
        0xe9, 8, 0, 0, 0, // jmp over embedded data
        0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01, // qword
        0x48, 0x8b, 0x05, 0xf1, 0xff, 0xff, 0xff, // mov rax, [rip - 15]
        0x48, 0x01, 0xc8, // add rax, rcx
    ];
    let (guest, _) = interpret(&code, 6, &[]);
    assert_eq!(guest.gpr[0], reference::move_and_jump());
    assert_eq!(guest.gpr[1], u64::MAX - 1);
    assert_eq!(guest.rip, Addr(ENTRY.0 + code.len() as u64));
}

#[test]
fn call_pushes_the_return_pc_and_ret_restores_the_stack() {
    for initial in EDGE_VALUES {
        #[cfg(target_arch = "x86_64")]
        let (expected_rax, expected_flags) = {
            let (rax, flags, correct_return, balanced_stack) = reference::call_and_return(initial);
            assert!(correct_return && balanced_stack);
            (rax, flags)
        };
        #[cfg(target_arch = "aarch64")]
        let (expected_rax, expected_flags) = reference::arithmetic(Arithmetic::Add, initial, 1);
        let stack = Addr(DATA.0 + 4096);
        let (guest, memory) = interpret(CALL_AND_RETURN, 4, &[(0, initial), (4, stack.0)]);
        assert_eq!(guest.gpr[0], expected_rax);
        assert_eq!(
            guest.rflags & ARITHMETIC_FLAGS,
            expected_flags & ARITHMETIC_FLAGS
        );
        assert_eq!(guest.gpr[4], stack.0);
        assert_eq!(
            u64::from_le_bytes(read_qword(&memory, Addr(stack.0 - 8))),
            ENTRY.0 + 5
        );
        assert_eq!(guest.rip, Addr(ENTRY.0 + CALL_AND_RETURN.len() as u64));
    }
}
