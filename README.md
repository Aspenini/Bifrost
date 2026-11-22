# Bifröst

**Run Beyond Boundaries**

Bifröst is a library-first Rust framework for userspace binary translation and OS compatibility. The raw-code SDK now runs a [documented x86-64 subset](docs/x86-64-subset.md) in a bounded interpreter over caller-owned guest memory.

## Run the fixtures

Install Rust via rustup. The workspace selects its pinned toolchain automatically.

```sh
cargo run --locked -p bifrost-example-execute-raw-x86
cargo run --locked -p bifrost-example-verify-ir
cargo run --locked -p bifrost-cli --features bifrost-runtime/guest-x86_64,bifrost-runtime/interp -- capabilities
```

The first example decodes, lifts, verifies and interprets four x86-64 instructions. It prints `RAX = 42, RIP = 0x1018`. The second prints a verified, hand-authored IR fixture.

| Crate | API |
| --- | --- |
| `bifrost-core` | Guest and host identities, CPU state, fault and trace types, capability records |
| `bifrost-memory` | Sparse guest pages, guarded reads, writes and fetches, page generations |
| `bifrost-ir` | Typed IR, verifier and printer |
| `bifrost-decoder-x86`, `bifrost-lifter-x86` | x86-64 frontend with normalized operands and scalar instruction semantics |
| `bifrost-interpreter` | Bounded evaluation of verified IR |
| `bifrost-runtime` | Capability discovery and feature-gated raw x86-64 dispatch |
| `bifrost-testkit` | Deterministic IR and x86-64 fixtures |

See the [execution model](docs/execution-model.md) and [capability matrix](docs/capability-matrix.md) for scope and feature requirements. The CLI reports capabilities. Executable loading, OS services, JIT, native fast paths, and the C ABI are later milestones.

## Verify the workspace

See [CONTRIBUTING.md](CONTRIBUTING.md) for the local checks and [SECURITY.md](SECURITY.md) for vulnerability reporting.

Bifröst's code is dual licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
