#![cfg(any(feature = "linux-elf", feature = "windows-pe"))]

use bifrost_runtime::{
    RuntimeProfile,
    core::ExecMode,
    process::{RunError, RunOptions},
};

const OPTIONS: RunOptions = RunOptions {
    execution: ExecMode::Interpreter,
    instruction_limit: 100,
};

#[cfg(feature = "linux-elf")]
#[test]
fn prepared_linux_process_retains_the_thread_and_runs_once() {
    use bifrost_runtime::process::{Io, Vfs};
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let vfs = Vfs::default();
    let image = include_bytes!("../../../tests/fixtures/linux/hello.elf").to_vec();
    let mut process = RuntimeProfile::default()
        .prepare_linux(
            &image,
            &[b"guest"],
            &[],
            Io {
                input: &[],
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
            &vfs,
            OPTIONS,
        )
        .unwrap();
    drop(image);
    let entry = process.metadata().entry;
    assert_eq!(process.thread().cpu.pc(), entry);
    assert_eq!(process.execution(), ExecMode::Interpreter);
    assert_eq!(process.run(&mut ()).unwrap(), 42);
    assert_ne!(process.thread().cpu.pc(), entry);
    assert!(matches!(process.run(&mut ()), Err(RunError::AlreadyRun)));
    drop(process);
    assert_eq!(stdout, b"Hello from Bifrost ELF!\n");
    assert!(stderr.is_empty());
}

#[cfg(feature = "windows-pe")]
#[test]
fn unsupported_import_fails_during_preparation() {
    use bifrost_runtime::process::{ApiPolicy, WindowsIo};
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let outcome = RuntimeProfile::default().prepare_windows(
        include_bytes!("../../../tests/fixtures/windows/unsupported.exe"),
        WindowsIo {
            stdout: &mut stdout,
            stderr: &mut stderr,
        },
        ApiPolicy::Portable,
        OPTIONS,
    );
    assert!(matches!(outcome, Err(RunError::Import { module, symbol })
        if module == "KERNEL32.dll" && symbol == "GetTickCount"));
}
