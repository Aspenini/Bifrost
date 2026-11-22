#![cfg(all(
    feature = "guest-x86_64",
    feature = "interp",
    feature = "jit-cranelift",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "windows")
))]

use bifrost_runtime::{
    core::{
        CpuState, GuestAddress as Addr, GuestThread, GuestThreadId, TraceEvent, TraceSink, X86State,
    },
    memory::{AddressSpace, GuestMemory, GuestMemoryRead, Protection},
    x86::{ExecutionBudget, Executor, SystemJitMemory, X86Interpreter, X86Jit},
};
use bifrost_testkit::x86::{ARITHMETIC, CALL_AND_RETURN, MOVE_AND_ADD};

const ENTRY: Addr = Addr(0x1000);
const STACK: Addr = Addr(0x2000);

#[derive(Default)]
struct Trace(Vec<TraceEvent>);
impl TraceSink for Trace {
    fn record(&mut self, event: TraceEvent) {
        self.0.push(event);
    }
}

fn space(code: &[u8], data: Option<Protection>) -> AddressSpace {
    let mut memory = AddressSpace::new(4096, 2).unwrap();
    memory.map(ENTRY, 4096, Protection::ReadWrite).unwrap();
    memory.write(ENTRY, code).unwrap();
    memory
        .protect(ENTRY, 4096, Protection::ReadExecute)
        .unwrap();
    if let Some(protection) = data {
        memory.map(STACK, 4096, protection).unwrap();
    }
    memory
}

fn compare(
    code: &[u8],
    count: u64,
    registers: &[(usize, u64)],
    flags: u64,
    data: Option<Protection>,
) {
    let mut state = X86State {
        rip: ENTRY,
        rflags: flags,
        ..Default::default()
    };
    for &(reg, value) in registers {
        state.gpr[reg] = value;
    }
    let initial = GuestThread {
        id: GuestThreadId(1),
        cpu: CpuState::X86_64(Box::new(state)),
        pending_fault: None,
    };
    let (mut jit_thread, mut interp_thread) = (initial.clone(), initial);
    let (mut jit_memory, mut interp_memory) = (space(code, data), space(code, data));
    let (mut jit_trace, mut interp_trace) = (Trace::default(), Trace::default());
    let budget = ExecutionBudget {
        instructions: count,
    };
    let interp =
        X86Interpreter::new(&mut interp_memory, &mut interp_trace).run(&mut interp_thread, budget);
    let jit = X86Jit::new(&mut jit_memory, &mut jit_trace, SystemJitMemory)
        .unwrap()
        .run(&mut jit_thread, budget);
    match (jit, interp) {
        (Ok(jit), Ok(interp)) => assert_eq!(jit, interp),
        (Err(jit), Err(interp)) => assert_eq!(jit.to_string(), interp.to_string()),
        (jit, interp) => panic!("execution mismatch for {code:02x?}: {jit:?} vs {interp:?}"),
    }
    assert_eq!(jit_thread, interp_thread, "state mismatch for {code:02x?}");
    assert_eq!(jit_trace.0, interp_trace.0);
    if data == Some(Protection::ReadWrite) {
        let (mut jit_data, mut interp_data) = ([0; 4096], [0; 4096]);
        jit_memory.read(STACK, &mut jit_data).unwrap();
        interp_memory.read(STACK, &mut interp_data).unwrap();
        assert_eq!(jit_data, interp_data);
    }
}

#[test]
fn scalar_and_branch_results_match_the_interpreter() {
    for case in ARITHMETIC {
        compare(case.bytes, 1, &[(0, case.lhs), (3, case.rhs)], 0x602, None);
    }
    for opcode in [0x72, 0x73, 0x74, 0x75, 0x7c, 0x7d] {
        for flags in [0, 1, 0x40, 0x80, 0x800, 0x880] {
            compare(&[opcode, 7], 1, &[], flags | 2, None);
            compare(&[0xf, opcode + 0x10, 7, 0, 0, 0], 1, &[], flags | 2, None);
        }
    }
}

#[test]
fn guest_memory_stack_and_faults_match_the_interpreter() {
    for program in [MOVE_AND_ADD, CALL_AND_RETURN] {
        compare(program, 4, &[(4, 0x3000)], 2, Some(Protection::ReadWrite));
    }
    compare(
        MOVE_AND_ADD,
        4,
        &[(4, 0x3001)],
        2,
        Some(Protection::ReadWrite),
    );
    compare(MOVE_AND_ADD, 4, &[(4, 0x3000)], 2, Some(Protection::Guard));
    compare(
        CALL_AND_RETURN,
        4,
        &[(4, 0x3000)],
        2,
        Some(Protection::Read),
    );
    compare(&[0xc3], 1, &[(4, 0x3000)], 2, None);
    compare(&[0x48, 0x8b, 0], 1, &[(0, 0x8000_0000_0000)], 2, None);
    compare(&[0x90], 2, &[], 2, None);
    for program in [&[0xf, 0xb][..], &[0xf, 5], &[0x66; 15]] {
        compare(program, 1, &[], 2, None);
    }
}
