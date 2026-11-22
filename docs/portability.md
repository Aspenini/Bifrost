# Portability

The core uses only Rust's standard library. Host identity comes from the Rust target. Host page size and executable-memory policy remain unknown until an adapter probes them.

The workflow configures Windows x86-64 and Linux x86-64 checks. ARM64 host execution and macOS are pending. The ARM64 CPU-state type describes guest data and is usable on any host that builds the starter workspace.
