# Portability

The core uses only Rust's standard library. Host identity comes from the Rust target. The CLI includes page-size probes for Windows, Linux, and macOS. On Windows and Linux, a JIT-enabled build also probes whether the system executable-memory adapter can publish code.

The CI matrix runs on Windows, Linux, and macOS on x86-64 and ARM64. Windows and Linux run the interpreter and JIT suites. macOS uses the interpreter. Cross-compiled Linux ARM64 binaries can also be run under QEMU user-mode emulation. Keep emulated results separate from native ARM64 results, particularly for instruction-cache coherence. [Native workspace CI](https://github.com/Aspenini/Bifrost/actions/runs/45068515511)
