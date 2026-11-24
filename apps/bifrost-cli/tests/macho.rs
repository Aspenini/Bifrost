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
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("requires a thin x86-64 Mach-O executable")
    );
}

#[test]
fn executes_standalone_macho_and_rejects_dyld_images() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/macho");
    let tool = env!("CARGO_BIN_EXE_bifrost");
    for mode in ["interp", "auto"] {
        let result = Command::new(tool)
            .args(["run", "--exec", mode])
            .arg(fixtures.join("standalone.macho"))
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(45),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, b"Hello from Bifrost Darwin!\n");
    }
    let result = Command::new(tool)
        .arg("run")
        .arg(fixtures.join("entry-x86_64.macho"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("standalone image"));
}
