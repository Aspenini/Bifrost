#![cfg(all(
    feature = "guest-x86_64",
    feature = "jit-cranelift",
    any(
        all(
            target_arch = "x86_64",
            any(target_os = "linux", target_os = "windows")
        ),
        all(target_arch = "aarch64", target_os = "linux")
    )
))]

use bifrost_runtime::{
    core::{AccessKind, CpuState, FaultClass, GuestAddress as Addr, GuestThread, GuestThreadId},
    memory::{AddressSpace, GuestMemory, Protection},
    x86::{CacheStats, ExitReason, SystemJitMemory, X86Jit},
};

const CODE: Addr = Addr(0x1000);
const NEXT_PAGE: Addr = Addr(0x1010);

fn space(immediate: u8) -> AddressSpace {
    let mut memory = AddressSpace::new(16, 3).unwrap();
    memory.map(CODE, 32, Protection::ReadWrite).unwrap();
    // mov rax, imm64, crossing the boundary at 0x1010; a separate NOP starts another block.
    memory
        .write(Addr(0x100e), &[0x48, 0xb8, immediate, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    memory.write(CODE, &[0x90]).unwrap();
    memory.protect(CODE, 32, Protection::ReadExecute).unwrap();
    memory
}

fn thread() -> GuestThread {
    GuestThread {
        cpu: CpuState::X86_64(Box::default()),
        id: GuestThreadId(1),
        pending_fault: None,
    }
}

fn rax(thread: &GuestThread) -> u64 {
    let CpuState::X86_64(state) = &thread.cpu else {
        panic!("expected an x86-64 thread");
    };
    state.gpr[0]
}

fn step(jit: &mut X86Jit<'_, SystemJitMemory>, thread: &mut GuestThread, at: Addr) -> ExitReason {
    thread.cpu.set_pc(at);
    thread.pending_fault = None;
    jit.step(thread).unwrap()
}

#[test]
fn invalidates_blocks_covering_a_changed_page_and_keeps_unrelated_blocks() {
    let mut memory = space(7);
    let mut trace = ();
    let mut jit = X86Jit::new(&mut memory, &mut trace, SystemJitMemory).unwrap();
    let mut thread = thread();
    let start = Addr(0x100e);
    for _ in 0..2 {
        assert_eq!(
            step(&mut jit, &mut thread, start),
            ExitReason::Dispatch(Addr(0x1018))
        );
        assert_eq!(rax(&thread), 7);
    }
    step(&mut jit, &mut thread, CODE);
    assert_eq!(
        jit.cache_stats(),
        CacheStats {
            hits: 1,
            misses: 2,
            resident: 2
        }
    );

    jit.memory_mut()
        .protect(NEXT_PAGE, 16, Protection::ReadWrite)
        .unwrap();
    let ExitReason::Fault(fault) = step(&mut jit, &mut thread, start) else {
        panic!("fetch should be denied");
    };
    assert_eq!(fault.pc, start);
    assert_eq!(fault.class, FaultClass::Protection);
    assert_eq!(fault.memory_access, Some((NEXT_PAGE, AccessKind::Execute)));
    assert_eq!(rax(&thread), 7);

    jit.memory_mut().write(NEXT_PAGE, &[9]).unwrap();
    jit.memory_mut()
        .protect(NEXT_PAGE, 16, Protection::ReadExecute)
        .unwrap();
    step(&mut jit, &mut thread, start);
    assert_eq!(rax(&thread), 9);
    step(&mut jit, &mut thread, CODE);
    assert_eq!(
        jit.cache_stats(),
        CacheStats {
            hits: 2,
            misses: 4,
            resident: 2
        }
    );

    jit.memory_mut().unmap(NEXT_PAGE, 16).unwrap();
    let ExitReason::Fault(fault) = step(&mut jit, &mut thread, start) else {
        panic!("fetch should be unmapped");
    };
    assert_eq!(fault.class, FaultClass::Unmapped);
    jit.memory_mut()
        .map(NEXT_PAGE, 16, Protection::ReadWrite)
        .unwrap();
    jit.memory_mut().write(NEXT_PAGE, &[11]).unwrap();
    jit.memory_mut()
        .protect(NEXT_PAGE, 16, Protection::ReadExecute)
        .unwrap();
    step(&mut jit, &mut thread, start);
    assert_eq!(rax(&thread), 11);
}

#[test]
fn replacing_an_address_space_cannot_reuse_matching_generations() {
    let mut memory = space(7);
    let replacement = space(12);
    assert_ne!(memory.id(), replacement.id());
    assert_eq!(
        memory.generation_at(NEXT_PAGE),
        replacement.generation_at(NEXT_PAGE)
    );
    let mut trace = ();
    let mut jit = X86Jit::new(&mut memory, &mut trace, SystemJitMemory).unwrap();
    let mut thread = thread();
    step(&mut jit, &mut thread, Addr(0x100e));
    assert_eq!(rax(&thread), 7);
    *jit.memory_mut() = replacement;
    step(&mut jit, &mut thread, Addr(0x100e));
    assert_eq!(rax(&thread), 12);
    assert_eq!(
        jit.cache_stats(),
        CacheStats {
            hits: 0,
            misses: 2,
            resident: 1
        }
    );
}

#[test]
fn enforces_capacity_and_can_disable_caching() {
    let mut memory = space(7);
    let mut trace = ();
    let mut jit = X86Jit::new(&mut memory, &mut trace, SystemJitMemory).unwrap();
    jit.set_cache_capacity(1);
    let mut thread = thread();
    for pc in [CODE, Addr(0x100e), CODE, CODE] {
        step(&mut jit, &mut thread, pc);
    }
    assert_eq!(
        jit.cache_stats(),
        CacheStats {
            hits: 1,
            misses: 3,
            resident: 1
        }
    );
    jit.set_cache_capacity(0);
    step(&mut jit, &mut thread, CODE);
    assert_eq!(
        jit.cache_stats(),
        CacheStats {
            hits: 1,
            misses: 4,
            resident: 0
        }
    );
}
