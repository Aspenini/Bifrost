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
| Cranelift JIT and executable-memory publisher | Partial, Windows x86-64 or Linux x86-64/ARM64 with `jit-cranelift` and executable mappings permitted |
| Native fast paths | Unavailable |
| Linux ELF and syscall provider | Partial, static x86-64 images, initial stack, read-only mounts and core syscalls. Enable `linux-elf`. |
| Windows PE and API provider | Partial, fixed-base x86-64 console images and named Win32/NT calls. Enable `windows-pe`. |
| Mach-O loading | Unavailable |
| ABI bridge | Partial, four guest ABI descriptors with fixed signatures and synchronous callbacks. Enable `abi-bridge`. |
| General host OS services and C ABI | Unavailable |
| CLI | Capability reporting, image inspection and bounded ELF/PE execution |

Run `cargo run --locked -p bifrost-cli --features "bifrost-runtime/guest-x86_64,bifrost-runtime/guest-aarch64,bifrost-runtime/interp,bifrost-runtime/jit-cranelift" -- capabilities` to check both guest profiles and backends. A reserved feature reports its compiled crate boundary as implemented=false, available=false, and validated=false.
