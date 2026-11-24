use std::{path::Path, process::Command};

#[test]
fn inspects_both_slices_of_a_universal_macho() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/macho/universal.macho");
    let output = Command::new(env!("CARGO_BIN_EXE_bifrost"))
        .arg("inspect")
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("universal (2 slices)"));
    assert!(report.contains("X86_64 entry 0x100000268"));
    assert!(report.contains("AArch64 entry 0x100000278"));
}

#[test]
fn inspects_macho_and_refuses_execution() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/macho/entry-arm64.macho");
    let tool = env!("CARGO_BIN_EXE_bifrost");
    let inspect = Command::new(tool)
        .arg("inspect")
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(inspect.status.success());
    assert!(String::from_utf8_lossy(&inspect.stdout).contains("MachO AArch64 entry 0x100000278"));
    let run = Command::new(tool)
        .arg("run")
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("macOS guest execution is unavailable"));
}
