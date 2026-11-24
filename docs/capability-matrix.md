# Capability matrix

| Capability | Current status |
| --- | --- |
| x86-64 and AArch64 guest CPU records | Partial, identity and register snapshots |
| Guest memory | Partial, sparse mapped pages and checked byte access |
| IR v1 | Partial, typed scalar operations and structural verification |
| x86-64 decoding and lifting | Partial, enable `guest-x86_64` |
| ARM64 decoding and lifting | Partial, enable `guest-aarch64` |
| Verified IR interpretation | Partial, enable `interp` |
| Bare x86-64 execution | Partial, enable `guest-x86_64` and `interp` or `jit-cranelift`. See the [instruction subset](x86-64-subset.md). |
| Bare ARM64 execution | Partial, enable `guest-aarch64` and `interp` or `jit-cranelift`. See the [instruction subset](arm64-subset.md). |
| Cranelift JIT and executable-memory publisher | Partial, Windows and Linux x86-64/ARM64 with `jit-cranelift` and executable mappings permitted |
| Native fast paths | Unavailable |
| Linux ELF and syscall provider | Partial, static x86-64 ET_EXEC and relative-relocation PIE images, initial stack, read-only mounts and core syscalls. Enable `linux-elf`. |
| Cooperative Linux threads | Rust embedding with host-enrolled x86-64 threads, guarded stacks, and interpreter/JIT parity |
| Windows PE and API provider | Partial, fixed-base x86-64 console images and named Win32/NT calls. Enable `windows-pe`. |
| Mach-O inspection | Experimental, thin and universal x86-64/ARM64 executables, plus ARM64e slice identification. See the [research scope](research.md). |
| ABI bridge | Partial, four guest ABI descriptors with fixed signatures and synchronous callbacks. Enable `abi-bridge`. |
| C ABI | Version 1 for raw x86-64 and ARM64 interpretation and optional portable ELF/PE execution. See the [C SDK](sdk.md). |
| CLI | Host diagnostics, text or JSON capability reporting, image inspection, bounded ELF/PE execution and JSONL tracing |

| Host | Execution path | Validation evidence |
| --- | --- | --- |
| Windows x86-64 | Interpreter, JIT, native PE console writes | ELF and PE fixtures and C example from the extracted release SDK |
| Linux x86-64 | Interpreter, JIT, portable PE provider | ELF and PE fixtures and C example from the extracted release SDK under WSL |
| Linux ARM64 | Interpreter, JIT, portable PE provider | Native workspace CI and extracted ELF SDK C example |
| Windows ARM64 | Interpreter, JIT, native PE console writes | Native workspace CI and extracted ELF SDK C example |
| macOS ARM64 | Interpreter and portable providers | Native workspace CI and extracted ELF SDK C example |
| macOS x86-64 | Interpreter and portable providers | Native workspace CI and extracted ELF SDK C example |

The native baseline results above are from commit `618b05e`. [Workspace CI](https://github.com/Aspenini/Bifrost/actions/runs/45068515511) · [SDK fixture tests](https://github.com/Aspenini/Bifrost/actions/runs/45068515506)

Rust embedders can enroll additional threads before starting a Linux guest. They run cooperatively with synthetic process and thread IDs and one shared instruction budget. Guest-created threads, signals, and Windows exception delivery are outside this profile. The CLI and C API use one guest thread. macOS and the C interface use the interpreter. [Thread lifecycle](execution-model.md)

Run `cargo run --locked -p bifrost-cli --features "bifrost-runtime/guest-x86_64,bifrost-runtime/guest-aarch64,bifrost-runtime/interp,bifrost-runtime/jit-cranelift" -- capabilities` to check both guest profiles and backends. A reserved feature reports its compiled crate boundary as implemented=false, available=false, and validated=false.

## Verify the Phase 7 runtime

```sh
cargo test --locked -p bifrost-runtime --all-features --test threads --test image_mutations
cargo test --locked -p bifrost-os-linux
cargo deny --locked --all-features check advisories licenses
```

The thread fixture tests independent stacks, shared memory, yield and preemption, and exit, fault, and budget behavior. Windows and Linux JIT hosts also compare the full scheduling trace against the interpreter. The image mutation test exercises inspection of bounded changes to the self-authored ELF, PE, and Mach-O fixtures. The workspace CI runs these tests on the six host profiles above.
