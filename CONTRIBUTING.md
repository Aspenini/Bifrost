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
cargo run --locked -p bifrost-example-execute-raw-x86 --features jit-cranelift
cargo run --locked -p bifrost-cli -- capabilities
```

`cargo test --locked -p bifrost-differential` compares the interpreter and JIT against test-only x86-64 assembly on Windows and Linux x86-64. Linux ARM64 uses the same fixtures and independent Rust expectations.

On an x86-64 Linux development host with the GNU ARM64 cross-toolchain, ARM64 sysroot and QEMU user-mode runner installed, run the ARM64 differential and JIT tests with the pinned cross target.

```sh
rustup target add aarch64-unknown-linux-gnu
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER='qemu-aarch64 -L /usr/aarch64-linux-gnu'
cargo test --locked --target aarch64-unknown-linux-gnu --release -p bifrost-differential
cargo test --locked --target aarch64-unknown-linux-gnu -p bifrost-runtime --all-features --test jit --test jit_cache
```

Scope changes to a crate or a narrow feature. Record cross-crate changes in an ADR, update the dependency allowlist with any new edge, and add a deterministic fixture for every newly claimed guest-visible behavior. Include both accepted and rejected inputs. Checked-in fixture bytes and IR are hand-authored for Bifröst.
