use std::{path::Path, process::Command};

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
