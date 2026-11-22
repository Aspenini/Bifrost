# Portability

The core uses only Rust's standard library. Host identity comes from the Rust target. Host page size and executable-memory policy remain unknown until an adapter probes them.

The CI matrix runs both scalar guest suites on Windows x86-64, Linux x86-64 and a native Linux ARM64 runner. Cross-compiled ARM64 binaries can also be run under QEMU user-mode emulation. Keep emulated results separate from native ARM64 results, particularly for instruction-cache coherence. macOS and Windows ARM64 host adapters are future work.
