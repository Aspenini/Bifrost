# Debugging

Run the verification example to view guest bytes and an IR listing tagged with guest PCs. IR errors expose a block ID, operation PC if applicable, and error kind. Memory errors report the first failing byte address and access kind for permission violations.

The raw x86-64 executor emits a `TraceEvent::Step` for each attempted instruction and a `TraceEvent::Fault` when a guest fault occurs. The fault reports the instruction PC and, for address faults, the failing guest address and access kind. The thread retains that fault. Unsupported valid instructions return a typed lift error carrying their opcode and PC.

`bifrost doctor` reports the host page size, executable-memory policy, and available backends and executable profiles. `bifrost version --verbose` shows the package version, target, and compiled JIT feature. Set `BIFROST_BUILD_REVISION` at build time to record a source revision.

```sh
bifrost trace tests/fixtures/linux/hello.elf --output hello.jsonl
```

The JSONL file uses schema 1. It starts with a schema record, follows with guest instruction and service-boundary PCs and typed faults, then records the exit status or an error marker. Guest stdout and stderr are independent of the trace. Trace output must name a new file. Paths, guest arguments and guest output are excluded from the records.

If a component is unavailable after enabling a feature, check the [capability matrix](capability-matrix.md).
