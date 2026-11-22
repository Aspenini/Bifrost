# Debugging

Run the verification example to view guest bytes and an IR listing tagged with guest PCs. IR errors expose a block ID, operation PC if applicable, and error kind. Memory errors report the first failing byte address and access kind for permission violations.

The raw x86-64 executor emits a `TraceEvent::Step` for each attempted instruction and a `TraceEvent::Fault` when a guest fault occurs. The fault reports the instruction PC and, for address faults, the failing guest address and access kind. The thread retains that fault. Unsupported valid instructions return a typed lift error carrying their opcode and PC.

If a component is unavailable after enabling a feature, check the [capability matrix](capability-matrix.md).
