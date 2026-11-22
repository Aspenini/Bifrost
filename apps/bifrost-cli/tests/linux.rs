use std::{path::Path, process::Command};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/linux")
        .join(name)
}

#[test]
fn inspects_and_runs_the_static_elf() {
    let path = fixture("hello.elf");
    let tool = env!("CARGO_BIN_EXE_bifrost");
    let inspect = Command::new(tool)
        .arg("inspect")
        .arg(&path)
        .output()
        .unwrap();
    assert!(inspect.status.success());
    assert!(String::from_utf8_lossy(&inspect.stdout).contains("Elf X86_64"));
    for mode in ["interp", "auto"] {
        let result = Command::new(tool)
            .args(["run", "--exec", mode])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(42),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(&result.stdout, b"Hello from Bifrost ELF!\n");
    }
}

#[test]
fn missing_syscall_reports_number_and_guest_pc() {
    let result = Command::new(env!("CARGO_BIN_EXE_bifrost"))
        .arg("run")
        .arg(fixture("unsupported.elf"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("syscall 999 at 0x"));
}
