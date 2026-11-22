# ADR 0001 — Starter public contracts

**Status** Accepted for the starter milestone.

Guest addresses are checked integer newtypes. CPU and target state are layered separately so profile validation cannot be confused with processor support. ABI identity is validated against guest OS and ISA. Calling and layout descriptors belong to the ABI milestone.

Guest memory methods borrow caller-owned slices, leave the caller's data or existing mappings intact on errors, and require exclusive mutation. The starter address space uses sparse, owned guest pages. This keeps page sizes independent of the host and makes byte-level behavior straightforward to test. Backends will need an explicit synchronization and cache policy before enabling parallel execution.

The read-only memory contract lives in core so lifters can borrow checked instruction fetches without depending on the concrete address space. A lifter emits unverified IR. The verifier consumes it and returns an immutable verified view. Code generators accept these verified views and retain responsibility for supported lowering and guest-visible effects. Schedulers and executors work through mutable thread borrows. Capability providers lend a read-only registry. The starter API evolves in Rust during 0.x. The C boundary is reserved until ownership and error handling have their own compatibility tests.
