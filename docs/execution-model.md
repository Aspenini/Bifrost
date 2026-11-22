# Execution model

The raw x86-64 SDK interprets the [declared scalar subset](x86-64-subset.md). Enable `guest-x86_64` and `interp` on `bifrost-runtime` to use `runtime::x86::X86Interpreter`. The caller owns guest memory and thread state, supplies the entry PC and optional tracing sink, and chooses how many instructions may run.

`step` fetches and lifts one instruction from executable guest pages, verifies the IR, and evaluates it. A successful step returns the new guest PC. `run` repeats this process until a guest fault, a typed execution error, or the budget is exhausted. Reaching the end of a buffer is caller-defined through a budget or stop PC. The next fetch follows address-space permissions. There is no code cache.

The standalone `bifrost-interpreter` executes verified IR directly. Each step covers the consecutive operations at one PC within a block, with the terminator included if it has that PC. Block parameters transfer as raw typed bits. Budget exits preserve the frame so a caller can resume with the same thread. Memory operations are ordered. A frontend that needs precise faults must place faulting checks and accesses before architectural writes for that instruction.

For a bare x86-64 guest with the scalar profile and enabled features, `Auto` resolves to the interpreter. Other guest environments, native execution, and JIT return `UnsupportedExecutionMode`. Host ISA identity alone cannot establish native execution compatibility.
