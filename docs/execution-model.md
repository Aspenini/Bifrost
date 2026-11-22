# Execution model

The raw x86-64 SDK runs the [declared scalar subset](x86-64-subset.md). Enable `guest-x86_64` with either `interp` for `bifrost_runtime::x86::X86Interpreter` or `jit-cranelift` for `bifrost_runtime::x86::X86Jit`. The caller owns guest memory and thread state, supplies the entry PC and optional trace sink, and sets the instruction budget.

A successful step returns the next guest PC. The runtime fetches one instruction from executable guest pages, lifts and verifies it, then interprets the IR or runs the corresponding host code. `run` repeats until a guest fault, a typed error, or exhaustion of the instruction budget. The next fetch after the last instruction in a buffer follows guest memory permissions.

The standalone `bifrost-interpreter` executes verified IR directly. Each step covers the consecutive operations at one PC within a block, with the terminator included if it has that PC. Block parameters transfer as raw typed bits and budget exits preserve the frame. An instruction frontend puts faulting checks and accesses before architectural writes to give precise restart state.

`RuntimeProfile::select_execution` resolves `Auto` to JIT when the bare scalar target and system executable-memory policy permit it, then to the interpreter if enabled. JIT publication errors remain typed during execution. The system publishers target Windows and Linux on x86-64. See [ADR 0002](adr/0002-cranelift-block-abi.md) for memory and exit handling.
