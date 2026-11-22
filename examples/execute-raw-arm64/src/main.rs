use bifrost_runtime::{
    core::{
        AbiId, Arm64State, CpuState, GuestAddress as Addr, GuestIsa, GuestOs, GuestTarget,
        GuestThread, GuestThreadId,
    },
    memory::{AddressSpace, GuestMemory, Protection},
    raw::{ExecutionBudget, Executor, ExitReason},
};
use bifrost_testkit::arm64::{MOVE_ADD_AND_CALL, MOVE_ADD_AND_CALL_STEPS, bytes};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = GuestTarget::new(
        GuestIsa::AArch64,
        GuestOs::None,
        AbiId::Bare,
        Default::default(),
    )?;
    let code = Addr(0x1000);
    let data = Addr(0x2000);
    let mut memory = AddressSpace::new(4096, 2)?;
    memory.map(code, 4096, Protection::ReadWrite)?;
    memory.write(code, &bytes(&MOVE_ADD_AND_CALL))?;
    memory.protect(code, 4096, Protection::ReadExecute)?;
    memory.map(data, 4096, Protection::ReadWrite)?;

    let mut cpu = Arm64State {
        pc: code,
        sp: Addr(0x3000),
        ..Default::default()
    };
    cpu.x[1] = data.0;
    let mut thread = GuestThread {
        cpu: CpuState::AArch64(Box::new(cpu)),
        id: GuestThreadId(1),
        pending_fault: None,
    };
    let budget = ExecutionBudget {
        instructions: MOVE_ADD_AND_CALL_STEPS,
    };
    let reason = execute(&target, &mut memory, &mut thread, budget)?;
    let CpuState::AArch64(cpu) = thread.cpu else {
        unreachable!("example uses ARM64");
    };
    assert_eq!(reason, ExitReason::BudgetExhausted);
    assert_eq!(cpu.x[0], 43);
    println!("X0 = {}, PC = {:#x}", cpu.x[0], cpu.pc.0);
    Ok(())
}

#[cfg(all(
    feature = "jit-cranelift",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "windows")
))]
fn execute(
    target: &GuestTarget,
    memory: &mut AddressSpace,
    thread: &mut GuestThread,
    budget: ExecutionBudget,
) -> Result<ExitReason, Box<dyn std::error::Error>> {
    use bifrost_runtime::raw::{RawJit, SystemJitMemory};
    println!("Cranelift JIT");
    Ok(RawJit::new(target, memory, &mut (), SystemJitMemory)?.run(thread, budget)?)
}

#[cfg(not(all(
    feature = "jit-cranelift",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "windows")
)))]
fn execute(
    target: &GuestTarget,
    memory: &mut AddressSpace,
    thread: &mut GuestThread,
    budget: ExecutionBudget,
) -> Result<ExitReason, Box<dyn std::error::Error>> {
    use bifrost_runtime::raw::RawInterpreter;
    if cfg!(feature = "jit-cranelift") {
        return Err(
            "the example's system executable-memory adapter is unavailable on this host".into(),
        );
    }
    Ok(RawInterpreter::new(target, memory, &mut ())?.run(thread, budget)?)
}
