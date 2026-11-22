use std::{fs, path::Path, process::Command};

const TOOL: &str = env!("CARGO_BIN_EXE_bifrost");

#[test]
fn doctor_reports_host_probes_and_backends() {
    let output = Command::new(TOOL).arg("doctor").output().unwrap();
    assert!(output.status.success());
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("Host page size") && report.contains("Executable memory"));
    assert!(report.contains("interpreter Available"));
    let output = Command::new(TOOL)
        .args(["version", "--verbose"])
        .output()
        .unwrap();
    let version = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success() && version.contains("Revision "));
}

#[test]
fn trace_keeps_guest_output_separate_and_preserves_existing_files() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/linux/hello.elf");
    let output_path =
        std::env::temp_dir().join(format!("bifrost-trace-{}.jsonl", std::process::id()));
    let cleanup = TraceFile(&output_path);
    let execute = || {
        Command::new(TOOL)
            .arg("trace")
            .arg(&fixture)
            .arg("--output")
            .arg(&output_path)
            .output()
            .unwrap()
    };
    let output = execute();
    assert_eq!(
        output.status.code(),
        Some(42),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"Hello from Bifrost ELF!\n");
    let events = fs::read_to_string(&output_path).unwrap();
    assert_eq!(
        events.lines().next(),
        Some(r#"{"event":"start","schema":1}"#)
    );
    assert!(events.contains(r#"{"event":"step","thread":1,"pc":"0x"#));
    assert_eq!(
        events.lines().last(),
        Some(r#"{"event":"exit","status":42}"#)
    );
    assert!(!execute().status.success());
    assert_eq!(fs::read_to_string(&output_path).unwrap(), events);
    drop(cleanup);
}

struct TraceFile<'a>(&'a Path);
impl Drop for TraceFile<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0);
    }
}
