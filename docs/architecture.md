# Architecture, starter milestone

Consumers can select the core state, IR, memory, codegen, scheduling, or host traits directly as Cargo dependencies. The runtime profile composes capability reporting. The CLI consumes the runtime and remains outside the library dependency graph.

`bifrost-core` has no library dependencies. IR, memory, and host contracts depend on core. Codegen consumes verified IR, while the executor contract works with guest thread state. The declared execution, loader, and OS feature flags select reserved components in the runtime. Capability reporting tracks compilation, implementation, host availability and validation independently.

See [ADR 0001](adr/0001-public-traits.md) for contract decisions and [IR v1](ir-spec.md) for the current verifier rules.
