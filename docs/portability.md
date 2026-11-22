# Portability

The core uses only Rust's standard library. Host identity comes from the Rust target. Host page size and executable-memory policy remain unknown until an adapter probes them.

The CI matrix includes Windows x86-64, Linux x86-64 and a native Linux ARM64 runner for the scalar x86-64 guest suite. The same ARM64 binaries can also be tested under QEMU user-mode emulation. Keep emulated results separate from native ARM64 results, particularly for instruction-cache coherence. ARM64 guest execution and macOS are pending.
