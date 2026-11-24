# Static Linux ELF profile

Run `cargo run -p bifrost-cli -- inspect tests/fixtures/linux/hello.elf` to inspect the fixture. Run `cargo run -p bifrost-cli -- run tests/fixtures/linux/hello.elf` to print `Hello from Bifrost ELF!` and exit with status 42. Use `--exec interp` or `--exec jit` before the executable to select a backend.

The loader accepts little-endian static x86-64 ELF64 executables with separate load pages. It maps guest permissions, zero-fills BSS, and builds a 16-byte-aligned initial stack with arguments, environment terminator and auxiliary vector. The SDK accepts explicit environment values. The CLI starts with an empty environment and stdin. Dynamic loaders, TLS, and libc startup are outside this profile. [System V process initialization](https://gitlab.com/x86-psABIs/x86-64-ABI/-/blob/master/x86-64-ABI/low-level-sys-info.tex)

The syscall provider handles `read`, `write`, `close`, `lseek`, `sched_yield`, `getpid`, `gettid`, `exit`, `exit_group`, and read-only `openat` with absolute paths. Mounted files support `SEEK_SET`, `SEEK_CUR`, and `SEEK_END`. Standard streams are nonseekable. Pass `--mount /guest/name=HOST_FILE` to expose a file explicitly. The guest receives copied bytes and has no direct host filesystem access. Unsupported calls report the Linux syscall number and guest PC. [x86-64 Linux syscall table](https://github.com/torvalds/linux/blob/master/arch/x86/entry/syscalls/syscall_64.tbl) · [Linux lseek semantics](https://man7.org/linux/man-pages/man2/lseek.2.html)

Build the self-authored fixtures from their adjacent assembly sources using `clang --target=x86_64-linux-gnu -c SOURCE -o fixture.o` and `ld.lld -static -z separate-loadable-segments --build-id=none -o IMAGE fixture.o`.

## Static position-independent executables

The loader also supports a limited static PIE profile. It accepts little-endian x86-64 ELF64 with `ET_DYN` and `DF_1_PIE`, applies symbol-free `R_X86_64_RELATIVE` relocations at a fixed guest load bias, and seals complete RELRO pages after relocation. Relocation destinations must be aligned slots in writable segments, and relocation values must refer to the loaded image. The profile excludes interpreters, dependencies, TLS, text relocations, and symbol resolution. The fixed bias is reproducible across runs. [x86-64 System V ABI](https://gitlab.com/x86-psABIs/x86-64-ABI/-/blob/master/x86-64-ABI/low-level-sys-info.tex)

```sh
cargo run --locked -p bifrost-cli -- run tests/fixtures/linux/pie.elf
```

The fixture prints `Hello from Bifrost PIE!` and exits with status 44. Rebuild it with `clang --target=x86_64-linux-gnu -c tests/fixtures/linux/pie.S -o pie.o` and `ld.lld -pie --no-dynamic-linker -z separate-loadable-segments --build-id=none -o tests/fixtures/linux/pie.elf pie.o`. Its data pointer depends on Bifröst applying the linker's relocation before entry.
