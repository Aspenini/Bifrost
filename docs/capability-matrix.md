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
| ELF, PE, Mach-O loading and guest OS providers | Unavailable |
| General host OS services, ABI bridge, C ABI | Unavailable |
| CLI | Capability reporting |

Run `cargo run --locked -p bifrost-cli --features "bifrost-runtime/guest-x86_64,bifrost-runtime/guest-aarch64,bifrost-runtime/interp,bifrost-runtime/jit-cranelift" -- capabilities` to check both guest profiles and backends. A reserved feature reports its compiled crate boundary as implemented=false, available=false, and validated=false.
