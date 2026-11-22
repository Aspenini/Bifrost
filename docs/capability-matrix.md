# Starter capability matrix

| Capability | Current status |
| --- | --- |
| x86-64 and AArch64 guest CPU records | Partial, identity and register snapshots |
| Guest memory | Partial, sparse mapped pages and checked byte access |
| IR v1 | Partial, scalar operations and structural verification |
| Decoder, lifter, interpreter, JIT, native mode | Unavailable |
| ELF, PE, Mach-O loading and guest OS providers | Unavailable |
| Host OS service adapters, ABI bridge, C ABI | Unavailable |
| CLI | Capability reporting |

Run `cargo run --locked -p bifrost-cli -- capabilities` to check the current build. A reserved feature reports its compiled crate boundary as implemented=false, available=false, and validated=false.
