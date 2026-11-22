# ABI and bridging

The starter target validates bare, Linux x86-64/AArch64, and Windows x64/ARM64 ABI identities. This validation checks the combination of names. ABI descriptors, argument and return layouts, marshaling, callbacks, and host libraries are planned for the bridge milestone.

The future bridge must marshal only registered signatures and validate every guest buffer in the address space. Guest addresses are never reinterpreted as host pointers.
