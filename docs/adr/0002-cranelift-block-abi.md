# ADR 0002 — Cranelift block ABI

**Status** Accepted for Phase 2.

The Cranelift backend compiles verified, single-instruction blocks for the baseline host ISA with the host's C calling convention. Machine code is returned as bytes with its guest span. Every host offset within that block maps to the same guest instruction. The backend rejects machine traps and unresolved relocations and limits the number of IR operations accepted per instruction.

A generated entry receives an owned frame of 64-bit slots for general registers, flags, and exit data. It returns a status to its caller. Guest control flow exits to the dispatcher through this frame, including branches within executable guest pages. No guest address is used as a host code pointer.

The first lowering milestone handles scalar operations and checks branch targets without guest memory access. Executable memory publication, checked guest-memory callbacks and interpreter equivalence tests follow in the execution milestone.
