# Static Linux ELF profile

Run `cargo run -p bifrost-cli -- inspect tests/fixtures/linux/hello.elf` to inspect the fixture. Run `cargo run -p bifrost-cli -- run tests/fixtures/linux/hello.elf` to print `Hello from Bifrost ELF!` and exit with status 42. Use `--exec interp` or `--exec jit` before the executable to select a backend.

The loader accepts little-endian static x86-64 ELF64 executables with separate load pages. It maps guest permissions, zero-fills BSS, and builds a 16-byte-aligned initial stack with arguments, environment terminator and auxiliary vector. The SDK accepts explicit environment values. The CLI starts with an empty environment and stdin. Dynamic loaders, TLS, and libc startup are outside this profile. [System V process initialization](https://gitlab.com/x86-psABIs/x86-64-ABI/-/blob/master/x86-64-ABI/low-level-sys-info.tex)

The syscall provider handles `read`, `write`, `close`, `sched_yield`, `getpid`, `gettid`, `exit`, `exit_group`, and read-only `openat` with absolute paths. Standard descriptors are reserved and guest mounts name individual read-only files. Pass `--mount /guest/name=HOST_FILE` to expose a file explicitly. The guest receives copied bytes and has no direct host filesystem access. Unsupported calls report the Linux syscall number and guest PC. [x86-64 Linux syscall table](https://github.com/torvalds/linux/blob/master/arch/x86/entry/syscalls/syscall_64.tbl)

Build the self-authored fixtures from their adjacent assembly sources using `clang --target=x86_64-linux-gnu -c SOURCE -o fixture.o` and `ld.lld -static -z separate-loadable-segments --build-id=none -o IMAGE fixture.o`.
