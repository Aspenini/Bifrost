# Bifröst

**Run Beyond Boundaries**

Bifröst is a library-first Rust framework for userspace binary translation and OS compatibility. The raw-code SDK runs documented [x86-64](docs/x86-64-subset.md) and [ARM64](docs/arm64-subset.md) scalar guest subsets with an interpreter or a Cranelift JIT over caller-owned guest memory.

## Run the fixtures

Install Rust via rustup. The workspace selects its pinned toolchain automatically.

```sh
cargo run --locked -p bifrost-example-execute-raw-x86
cargo run --locked -p bifrost-example-execute-raw-x86 --features jit-cranelift
cargo run --locked -p bifrost-example-execute-raw-arm64 --features jit-cranelift
cargo run --locked -p bifrost-example-host-function-bridge --features jit-cranelift
cargo run --locked -p bifrost-example-verify-ir
cargo run --locked -p bifrost-cli -- inspect tests/fixtures/linux/hello.elf
cargo run --locked -p bifrost-cli -- run tests/fixtures/linux/hello.elf
cargo run --locked -p bifrost-cli -- run --api portable tests/fixtures/windows/hello.exe
cargo run --locked -p bifrost-cli --features "bifrost-runtime/guest-x86_64,bifrost-runtime/guest-aarch64,bifrost-runtime/interp,bifrost-runtime/jit-cranelift" -- capabilities
```

The raw-code examples print `RAX = 42, RIP = 0x1018` for x86-64 and `X0 = 43, PC = 0x1034` for ARM64. Both use the same runtime API, and both support JIT execution on Windows x86-64 and Linux x86-64/ARM64. The verification example prints a hand-authored IR fixture.

| Crate | API |
| --- | --- |
| `bifrost-core` | Guest and host identities, CPU state, fault and trace types, capability records |
| `bifrost-memory` | Sparse guest pages, guarded reads, writes and fetches, page generations |
| `bifrost-ir` | Typed IR, verifier and printer |
| `bifrost-decoder-x86`, `bifrost-lifter-x86` | x86-64 frontend with normalized operands and scalar instruction semantics |
| `bifrost-decoder-arm64`, `bifrost-lifter-arm64` | ARM64 scalar frontend, NZCV and ordered data accesses |
| `bifrost-interpreter` | Bounded evaluation of verified IR |
| `bifrost-codegen-clif` | Native lowering, guest-PC maps and checked exits |
| `bifrost-runtime` | Capability discovery and shared raw execution API |
| `bifrost-abi`, `bifrost-bridge` | Four guest call descriptors, typed native bindings and synchronous callbacks |
| `bifrost-testkit` | Deterministic IR, x86-64 and ARM64 fixtures |

See the [execution model](docs/execution-model.md), [ABI bridging](docs/abi-bridging.md), and [capability matrix](docs/capability-matrix.md) for scope and feature requirements. The CLI and [C SDK](docs/sdk.md) run the documented [static Linux ELF](docs/linux-elf.md) and [fixed-base Windows PE](docs/windows-pe.md) subsets.

## Verify the workspace

See [CONTRIBUTING.md](CONTRIBUTING.md) for the local checks and [SECURITY.md](SECURITY.md) for vulnerability reporting.

Bifröst's code is dual licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
