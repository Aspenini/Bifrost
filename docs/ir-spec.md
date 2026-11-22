# Bifröst IR v1

This is the starter subset of the v1 format. It is an in-memory Rust representation.

- Blocks declare typed parameters. Each local ID is defined once, and operands refer to definitions earlier in the same block or to its parameters. Edges pass one argument per target parameter with the same type. Every block is reachable from the entry. Loops are permitted and will require an execution budget.
- Each instruction and exit carries its guest PC. PCs are nondecreasing within each block and lie inside its checked byte span.
- Constants carry raw bits and fit their declared widths. Integer arithmetic wraps modulo the result width. Comparisons use the selected signedness. Shifts normalize the unsigned count according to the specified mask or modulo rule. Oversized logical shifts produce zero. Oversized arithmetic shifts fill with the sign bit. Rotates reduce the normalized count modulo width. Integer operations do not change flags by themselves.
- Architectural register operations use full-width registers checked against the target ISA. Guest addresses are a distinct type and convert explicitly to and from `I64`. `IntCast` truncates or zero-extends integer bits to a new width.
- Explicit feature requirements must be present in the declared guest CPU profile. The verifier checks the declaration. A backend must also determine whether it supports the resulting IR.
- A memory guard captures a guest address in a block-local value and pairs exactly once with a load or store at the same guest PC. The paired access must match type, alignment, endianness and access kind. Runtime checks happen at the access, including alignment, address overflow, mapping and permissions. Backend evaluation must preserve operation order and report the guest PC on failure.
- `Fence` orders earlier and later data accesses. The interpreter uses a sequentially consistent fence, and Cranelift lowers it to a host fence. The raw runtime serializes execution over an exclusively borrowed address space.
- `CheckTarget` validates a guest instruction address before side effects. A conditional check uses an `I1` value to limit the check to the taken edge.
- Exits branch within the function, return a next guest PC to the dispatcher, or produce a typed fault or trap. The format currently exposes no host-call operation.

Floating-point and vector values are available as raw bits. Arithmetic for those types, atomics, concurrent guest memory ordering, host calls, and deoptimization are future work.
