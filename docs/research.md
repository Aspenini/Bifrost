# Phase 8 research

The first experiment inspects thin, little-endian x86-64 and ARM64 Mach-O executables. It validates command bounds, segment tables, applicable protections, and that `LC_MAIN` points into file-backed executable `__TEXT`. It reads metadata without mapping guest pages, loading dependencies, or executing the image.

```sh
cargo run --locked -p bifrost-cli -- inspect tests/fixtures/macho/entry-arm64.macho
```

The fixture reports entry address `0x100000278`. The inspection profile requires `LC_MAIN`. Fat containers, legacy thread-state entry commands and ARM64e are outside the profile. The [Apple XNU Mach-O header](https://github.com/apple-oss-distributions/xnu/blob/main/EXTERNAL_HEADERS/mach-o/loader.h) defines the format fields this parser reads.

Rebuild the self-authored fixtures with LLVM from the repository root.

```powershell
New-Item -ItemType Directory -Force target/guest-fixtures
foreach ($arch in 'x86_64', 'arm64') {
    clang --target="$arch-apple-darwin" -c tests/fixtures/macho/entry.S -o "target/guest-fixtures/macho-$arch.o"
    ld64.lld -arch $arch -platform_version macos 13.0 13.0 -no_uuid -e _main -o "tests/fixtures/macho/entry-$arch.macho" "target/guest-fixtures/macho-$arch.o"
    llvm-objdump --macho --private-headers "tests/fixtures/macho/entry-$arch.macho"
}
```

The specification lists optional research projects with separate scopes.

| Research area | First verification required |
| --- | --- |
| macOS guest execution | Establish startup and dylib policy, then compare self-authored guest fixtures on macOS host hardware. |
| ARM64e | Establish pointer authentication and ABI behavior against native fixtures. |
| Advanced Linux userspace | Verify relocations and guest linker state with self-authored position-independent fixtures before extending syscall coverage. |
| x86-32 or RISC-V and additional backends | Establish a declared instruction subset and a deterministic differential corpus. |
| Android hosts | Check target page sizes, executable memory policy, and the interpreter on an actual device. |
| Dynamic plugins | Define a versioned interface, lifetime rules, and dependency policy. |
| Persistent cache | Verify key completeness and stale-code rejection across engine versions, host features, and process restarts. |
| Native execution | Define checked transitions and state transfer, and compare results and faults with the interpreter. |
