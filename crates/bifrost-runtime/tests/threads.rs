#![cfg(feature = "linux-elf")]

use bifrost_runtime::{
    RuntimeProfile,
    core::{ExecMode, FaultClass, GuestAddress, GuestThreadId, TraceEvent, TraceSink},
    process::{Io, PreparedProcess, RunError, RunOptions, ThreadStatus, Vfs},
};

const IMAGE: &[u8] = include_bytes!("../../../tests/fixtures/linux/threads.elf");
const WORKER: u64 = 128;
const GROUP_EXIT: u64 = 256;
const FAULT: u64 = 288;
const SPIN: u64 = 320;

#[derive(Default)]
struct Trace(Vec<TraceEvent>);
impl TraceSink for Trace {
    fn record(&mut self, event: TraceEvent) {
        self.0.push(event);
    }
}

fn with_process<T>(
    image: &[u8],
    mode: ExecMode,
    limit: u64,
    test: impl FnOnce(PreparedProcess<'_>) -> T,
) -> T {
    let mut out = std::io::sink();
    let mut err = std::io::sink();
    let vfs = Vfs::default();
    let process = RuntimeProfile::default()
        .prepare_linux(
            image,
            &[b"threads"],
            &[],
            Io {
                input: &[],
                stdout: &mut out,
                stderr: &mut err,
            },
            &vfs,
            RunOptions {
                execution: mode,
                instruction_limit: limit,
            },
        )
        .unwrap();
    test(process)
}

fn register(process: &mut PreparedProcess<'_>, offset: u64, arg: u64) -> GuestThreadId {
    process
        .add_linux_thread(process.metadata().entry.checked_add(offset).unwrap(), arg)
        .unwrap()
}

fn interleaved_run(mode: ExecMode) -> Vec<TraceEvent> {
    with_process(IMAGE, mode, 100, |mut process| {
        assert!(matches!(
            process.add_linux_thread(GuestAddress(0), 0),
            Err(RunError::ThreadEntry(_))
        ));
        assert_eq!(register(&mut process, WORKER, 77), GuestThreadId(2));
        let stack_pointers: Vec<_> = process
            .threads()
            .map(|(thread, _)| thread.cpu.sp())
            .collect();
        assert_ne!(stack_pointers[0], stack_pointers[1]);
        let mut trace = Trace::default();
        assert_eq!(process.run(&mut trace).unwrap(), 42);
        assert_eq!(
            process
                .threads()
                .map(|(_, status)| status)
                .collect::<Vec<_>>(),
            [ThreadStatus::Exited(42), ThreadStatus::Exited(77)],
        );
        let mut turns = trace
            .0
            .iter()
            .filter_map(|event| match event {
                TraceEvent::Step { thread, .. } => Some(*thread),
                _ => None,
            })
            .collect::<Vec<_>>();
        turns.dedup();
        assert_eq!(
            turns,
            [
                GuestThreadId(1),
                GuestThreadId(2),
                GuestThreadId(1),
                GuestThreadId(2)
            ]
        );
        trace.0
    })
}

#[test]
fn linux_threads_share_memory_but_keep_stacks_and_statuses_separate() {
    interleaved_run(ExecMode::Interpreter);
}

#[cfg(all(
    feature = "jit-cranelift",
    any(target_os = "windows", target_os = "linux"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
fn scheduling_trace_matches_the_jit() {
    assert_eq!(
        interleaved_run(ExecMode::Interpreter),
        interleaved_run(ExecMode::Jit)
    );
}

#[test]
fn quantum_preempts_a_spinner_and_group_exit_stops_it() {
    let mut image = IMAGE.to_vec();
    let entry = bifrost_runtime::process::inspect(IMAGE).unwrap().entry.0;
    // ELF64 e_entry. The spinning leader never calls sched_yield.
    image[24..32].copy_from_slice(&(entry + SPIN).to_le_bytes());
    with_process(&image, ExecMode::Interpreter, 66, |mut process| {
        assert_eq!(
            process
                .add_linux_thread(GuestAddress(entry + GROUP_EXIT), 55)
                .unwrap(),
            GuestThreadId(2)
        );
        let mut trace = Trace::default();
        assert_eq!(process.run(&mut trace).unwrap(), 55);
        assert_eq!(trace.0.len(), 66);
        assert!(trace.0[..64].iter().all(|event| matches!(
            event,
            TraceEvent::Step {
                thread: GuestThreadId(1),
                ..
            }
        )));
        assert_eq!(
            process
                .threads()
                .map(|(_, status)| status)
                .collect::<Vec<_>>(),
            [ThreadStatus::Stopped, ThreadStatus::Exited(55)]
        );
    });
}

#[test]
fn fault_and_budget_stop_the_whole_session() {
    with_process(IMAGE, ExecMode::Interpreter, 5, |mut process| {
        register(&mut process, WORKER, 77);
        let mut trace = Trace::default();
        assert!(matches!(process.run(&mut trace), Err(RunError::Budget)));
        assert_eq!(trace.0.len(), 5);
        assert!(matches!(
            trace.0[4],
            TraceEvent::Step {
                thread: GuestThreadId(2),
                ..
            }
        ));
        assert!(
            process
                .threads()
                .all(|(_, status)| status == ThreadStatus::Stopped)
        );
    });
    with_process(IMAGE, ExecMode::Interpreter, 100, |mut process| {
        register(&mut process, FAULT, 0);
        let Err(RunError::Fault(fault)) = process.run(&mut ()) else {
            panic!("expected the worker to fault");
        };
        assert_eq!(fault.thread, GuestThreadId(2));
        assert_eq!(fault.class, FaultClass::IllegalInstruction);
        assert_eq!(
            process.threads().nth(1).unwrap().0.pending_fault,
            Some(fault)
        );
    });
}
