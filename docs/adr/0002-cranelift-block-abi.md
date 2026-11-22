# ADR 0002 — Cranelift block ABI

**Status** Accepted for Phase 2.

The Cranelift backend compiles verified, single-instruction blocks for the baseline host ISA with the host's C calling convention. Machine code is returned as bytes with its guest span. Every host offset within that block maps to the same guest instruction. The backend rejects machine traps and unresolved relocations and limits the number of IR operations accepted per instruction.

A generated entry receives a bounded frame of 64-bit slots for general registers, flags, and exit data. It also receives one checked memory callback and an opaque context for the guarded guest memory interface. The callback applies the interpreter's span and alignment rules before asking the guest memory implementation for bytes. A helper failure exits immediately and reports the guest instruction PC. A panic from a Rust memory implementation is caught in the helper and resumes at the Rust caller after the generated code returns.

The host adapters publish fresh writable, non-executable mappings as executable, read-only images. Windows flushes the instruction cache before exposing the image. Linux ARM64 cleans and invalidates cache lines as required by `CTR_EL0` and synchronizes the instruction stream. The dispatcher's entry wrapper calls only code emitted by Bifröst. Guest branch destinations and data addresses never become host pointers. [Arm cache maintenance guidance](https://developer.arm.com/community/arm-community-blogs/b/architectures-and-processors-blog/posts/caches-self-modifying-code-implementing-clear-cache)

The ABI covers x86-64 and ARM64 scalar blocks with at most 64-bit memory accesses and no unresolved internal edges or parameters. The shared frame includes 32 general register slots, including the ARM64 SP slot, and an ISA-specific flags value. The published block checks the guest ISA and PC before entry. The guest ISA also selects address checks in the memory callback. Remaining IR forms fail at compilation.
