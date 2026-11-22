use bifrost_runtime::{
    core::{CpuState, GuestAddress, GuestThread, GuestThreadId, X86State},
    memory::{AddressSpace, GuestMemory, Protection},
    x86::{ExecutionBudget, Executor, ExitReason},
};
use bifrost_testkit::x86::MOVE_AND_ADD;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let entry = GuestAddress(0x1000);
    let mut memory = AddressSpace::new(4096, 2)?;
    memory.map(entry, 4096, Protection::ReadWrite)?;
    memory.write(entry, MOVE_AND_ADD)?;
    memory.protect(entry, 4096, Protection::ReadExecute)?;
    memory.map(GuestAddress(0x2000), 4096, Protection::ReadWrite)?;

    let mut cpu = X86State {
        rip: entry,
        ..X86State::default()
    };
    cpu.gpr[4] = 0x3000;
    let mut thread = GuestThread {
        id: GuestThreadId(1),
        cpu: CpuState::X86_64(Box::new(cpu)),
        pending_fault: None,
    };
    let reason = execute(&mut memory, &mut thread)?;
    let CpuState::X86_64(cpu) = thread.cpu else {
        unreachable!("example uses x86-64");
    };
    assert_eq!(reason, ExitReason::BudgetExhausted);
    assert_eq!(cpu.gpr[0], 42);
    println!("RAX = {}, RIP = {:#x}", cpu.gpr[0], cpu.rip.0);
    Ok(())
}

#[cfg(all(
    feature = "jit-cranelift",
    target_arch = "x86_64",
    any(target_os = "windows", target_os = "linux")
))]
fn execute(
    memory: &mut AddressSpace,
    thread: &mut GuestThread,
) -> Result<ExitReason, Box<dyn std::error::Error>> {
    use bifrost_runtime::x86::{SystemJitMemory, X86Jit};
    println!("Cranelift JIT");
    Ok(X86Jit::new(memory, &mut (), SystemJitMemory)?
        .run(thread, ExecutionBudget { instructions: 4 })?)
}

#[cfg(not(all(
    feature = "jit-cranelift",
    target_arch = "x86_64",
    any(target_os = "windows", target_os = "linux")
)))]
fn execute(
    memory: &mut AddressSpace,
    thread: &mut GuestThread,
) -> Result<ExitReason, Box<dyn std::error::Error>> {
    use bifrost_runtime::x86::X86Interpreter;
    if cfg!(feature = "jit-cranelift") {
        return Err(
            "the example's system executable-memory adapter is unavailable on this host".into(),
        );
    }
    Ok(X86Interpreter::new(memory, &mut ()).run(thread, ExecutionBudget { instructions: 4 })?)
}
