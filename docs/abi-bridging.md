# ABI bridging

Bifröst's typed bridge dispatches explicitly registered native entry addresses from either raw execution backend. The runtime pairs a bare CPU execution target with a separate guest call descriptor. This allows a Windows x64 or ARM64 calling convention in a raw fixture without an OS loader.

| Guest ABI profile | Scalar placement | Pair64 argument and result |
| --- | --- | --- |
| System V AMD64 | Separate integer and floating registers, then stack | Two integer slots, with whole-argument stack spill when needed |
| Windows x64 | Four shared argument positions and 32-byte shadow space | Argument by 16-byte-aligned guest copy. Hidden first result pointer echoed in RAX |
| AAPCS64 | Separate banks of eight integer and floating registers, then stack | Two integer slots or a whole stack argument |
| Windows ARM64 | Fixed-signature AAPCS64 register placement with a reserved X18 | Same fixed-signature integer-pair layout |

The descriptors also record stack alignment, preserved registers, red zones, variadic rules and unwind convention. Marshaling covers the fixed signatures declared by `NativeFunction`. Variadic calls, vector arguments, general aggregate classification and cross-boundary unwinding are separate work. [System V AMD64](https://gitlab.com/x86-psABIs/x86-64-ABI) · [Windows x64](https://learn.microsoft.com/en-us/cpp/build/x64-calling-convention) · [AAPCS64](https://github.com/ARM-software/abi-aa/blob/main/aapcs64/aapcs64.rst) · [Windows ARM64](https://learn.microsoft.com/en-us/cpp/build/arm64-windows-abi-conventions)

`BridgeRegistry` groups named symbols and allowlisted callback addresses by module. One registry owns each guest process's handles. A native call can resolve only a handle of the declared type owned by its module. Closed handles are never reused. A buffer call checks the full guest span and permissions, then supplies a temporary, size-limited host allocation with explicit copy-back. Guest addresses remain integers throughout marshaling.

The synchronous unary callback trampoline has a calling-thread lifetime. It runs only allowlisted executable guest entries. Each nesting level gets a caller-reserved stack frame, and callback execution shares the outer instruction budget. The executor checks stack balance at the return marker and reports the original guest PC on a callback fault. A fault leaves the outer CPU at its native binding. Guest memory writes from callback instructions and native side effects can persist when a later callback fails. Trusted native registrations supply fixed C-ABI functions and follow the pointer lifetime contracts.

The supported host column is the compiling process's native C ABI: Windows x86-64, Linux x86-64, or Linux ARM64. The conformance demo exercises all four guest ABIs on each configured host with both the interpreter and Cranelift. Native ARM64 CI is configured. Cross-test commands for QEMU are in [CONTRIBUTING.md](../CONTRIBUTING.md).

```sh
cargo run --locked -p bifrost-example-host-function-bridge
cargo run --locked -p bifrost-example-host-function-bridge --features jit-cranelift
cargo test --locked -p bifrost-example-host-function-bridge --all-features
```
