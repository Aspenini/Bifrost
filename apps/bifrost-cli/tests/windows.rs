use std::{path::Path, process::Command};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/windows")
        .join(name)
}

#[test]
fn inspects_and_runs_the_fixed_pe() {
    let path = fixture("hello.exe");
    let tool = env!("CARGO_BIN_EXE_bifrost");
    let inspected = Command::new(tool)
        .arg("inspect")
        .arg(&path)
        .output()
        .unwrap();
    assert!(inspected.status.success());
    let imports = String::from_utf8(inspected.stdout).unwrap();
    assert!(imports.contains("Pe X86_64"));
    assert!(imports.contains("KERNEL32.dll!WriteFile"));
    let mut modes = vec![("interp", "portable"), ("auto", "auto")];
    if cfg!(windows) {
        modes.push(("interp", "native"));
    }
    for (executor, api) in modes {
        let result = Command::new(tool)
            .args(["run", "--exec", executor, "--api", api])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(43),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(&result.stdout, b"Hello from Bifrost PE!\r\n");
    }
}

#[test]
fn missing_import_reports_module_and_symbol() {
    let result = Command::new(env!("CARGO_BIN_EXE_bifrost"))
        .arg("run")
        .arg(fixture("unsupported.exe"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("unsupported PE import KERNEL32.dll!GetTickCount")
    );
}
