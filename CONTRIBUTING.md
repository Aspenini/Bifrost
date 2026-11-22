# Contributing

Use the pinned Rust toolchain from rustup. The x86-64 decoder uses iced-x86 (MIT), with only the decoder and standard-library features enabled. Cargo.lock pins its dependencies.

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace
cargo test --locked --workspace --all-features
cargo doc --locked --workspace --all-features --no-deps
cargo check --locked -p bifrost-runtime --no-default-features
cargo run --locked -p xtask -- check-deps
cargo run --locked -p bifrost-example-verify-ir
cargo run --locked -p bifrost-example-execute-raw-x86
cargo run --locked -p bifrost-cli -- capabilities
```

On x86-64 hosts, `cargo test --locked -p bifrost-differential` compares the interpreter against assembly compiled into the test executable. The native reference code uses fixed inputs and a test-only assembly module.

Scope changes to a crate or a narrow feature. Record cross-crate changes in an ADR, update the dependency allowlist with any new edge, and add a deterministic fixture for every newly claimed guest-visible behavior. Include both accepted and rejected inputs. Checked-in fixture bytes and IR are hand-authored for Bifröst.
