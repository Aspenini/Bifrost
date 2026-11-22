# Bifröst

**Run Beyond Boundaries**

Bifröst is a library-first Rust framework for userspace binary translation and OS compatibility. This repository implements the starter milestone. You can assemble and verify hand-authored IR, model x86-64 or AArch64 guest state, and manipulate a permission-checked guest address space.

## Run the starter fixture

Install Rust via rustup. The workspace selects its pinned toolchain automatically.

```sh
cargo run --locked -p bifrost-example-verify-ir
cargo run --locked -p bifrost-cli -- capabilities
```

The example reads the scalar `41` from a mapped guest page and prints verified IR for a guarded load, an addition, a register write, and a dispatcher exit. It verifies the IR without executing it.

| Crate | Starter API |
| --- | --- |
| `bifrost-core` | Guest and host identities, CPU state, fault and trace types, capability records |
| `bifrost-memory` | Sparse guest pages, guarded reads, writes and fetches, page generations |
| `bifrost-ir` | Typed IR builder data, verifier, and printer |
| `bifrost-codegen`, `bifrost-executor`, `bifrost-host` | Backend, dispatch, and host-service contracts |
| `bifrost-runtime` | Capability discovery and typed rejection of unavailable execution modes |
| `bifrost-testkit` | Deterministic scalar IR and memory fixtures |

Decoders, lifters, execution backends, binary loaders, OS services, and the C ABI remain reserved for later phases. See the [capability matrix](docs/capability-matrix.md), [architecture](docs/architecture.md), and [IR specification](docs/ir-spec.md).

## Verify the workspace

See [CONTRIBUTING.md](CONTRIBUTING.md) for the local checks and [SECURITY.md](SECURITY.md) for vulnerability reporting.

Bifröst's code is dual licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
