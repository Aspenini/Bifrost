# Execution model

## Preparing executable sessions

Rust embedders can reuse a `RuntimeProfile` to call `prepare_linux` or `prepare_windows` explicitly. Preparation selects the backend, validates image metadata, resolves PE imports, and maps a private guest address space. It returns a `PreparedProcess` before the first guest instruction. Loaded bytes are copied into guest memory, so the caller may release the source image after preparation.

Call `PreparedProcess::run` with a trace sink. The session retains its selected backend and thread snapshot after exit, fault, or budget exhaustion. A prepared session runs once, with a single initial thread and an instruction budget. The `run_linux` and `run_windows` convenience functions use the same session API.

`CpuState::encode_snapshot` and `CpuState::decode_snapshot` preserve modeled x86-64 and ARM64 registers, including vector register bits. Version 1 starts with `BFRS`, a version byte, an ISA byte (1 for x86-64 or 2 for ARM64), and two reserved zero bytes. The payload contains the fields of the corresponding CPU state in declaration order, with no padding and all numeric fields in little-endian order. The decoder requires the exact payload size. These snapshots cover CPU registers. Memory, provider state, and scheduling are separate.

## Raw execution

The raw SDK runs the documented [x86-64](x86-64-subset.md) and [ARM64](arm64-subset.md) scalar subsets. Enable the required guest feature and either `interp` or `jit-cranelift`. Both `bifrost_runtime::raw::RawInterpreter` and `bifrost_runtime::raw::RawJit` select the frontend from a bare `GuestTarget`. The caller owns guest memory and thread state, supplies the entry PC and optional trace sink, and sets the instruction budget. The `x86` module also supplies convenience constructors for older callers.

A successful step returns the next guest PC. The runtime fetches one instruction from executable guest pages, lifts and verifies it, then interprets the IR or runs the corresponding host code. `run` repeats until a guest fault, a typed error, or exhaustion of the instruction budget. The next fetch after the last instruction in a buffer follows guest memory permissions.

The standalone `bifrost-interpreter` executes verified IR directly. Each step covers the consecutive operations at one PC within a block, with the terminator included if it has that PC. Block parameters transfer as raw typed bits and budget exits preserve the frame. An instruction frontend puts faulting checks and accesses before architectural writes to give precise restart state.

`RuntimeProfile::select_execution` resolves `Auto` to JIT when the bare scalar target and system executable-memory policy permit it, then to the interpreter if enabled. JIT publication errors remain typed during execution. The system publishers target Windows and Linux on x86-64 and ARM64. See [ADR 0002](adr/0002-cranelift-block-abi.md) for memory and exit handling.

The JIT cache holds up to 256 published blocks per executor by default. A lookup uses the address-space identity, guest PC, and every code-page generation touched by the instruction. Guest ISA, feature profile, compiler version and lowering options are fixed by the executor instance. A replaced address space clears the cache. A changed, protected or unmapped code page causes the affected block to be discarded before it can run. Exclusive access to `AddressSpace` covers both the generation check and execution. Use `set_cache_capacity(0)` to run without caching and `cache_stats()` to observe reuse.
