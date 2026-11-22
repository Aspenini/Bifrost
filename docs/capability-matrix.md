# Capability matrix

| Capability | Current status |
| --- | --- |
| x86-64 and AArch64 guest CPU records | Partial, identity and register snapshots |
| Guest memory | Partial, sparse mapped pages and checked byte access |
| IR v1 | Partial, typed scalar operations and structural verification |
| x86-64 decoding and lifting | Partial, enable `guest-x86_64` |
| Verified IR interpretation | Partial, enable `interp` |
| Bare x86-64 execution | Partial, enable both features. See the [instruction subset](x86-64-subset.md). |
| AArch64 decoding/lifting, JIT, native fast paths | Unavailable |
| ELF, PE, Mach-O loading and guest OS providers | Unavailable |
| Host OS service adapters, ABI bridge, C ABI | Unavailable |
| CLI | Capability reporting |

Run `cargo run --locked -p bifrost-cli --features bifrost-runtime/guest-x86_64,bifrost-runtime/interp -- capabilities` for the feature-enabled profile. A reserved feature reports its compiled crate boundary as implemented=false, available=false, and validated=false.
