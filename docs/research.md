# Phase 8 research

The Mach-O inspector accepts thin little-endian x86-64 and ARM64 executables and 32-bit or 64-bit big-endian universal directories containing those slices. It checks directory bounds, alignment, overlap, and consistency with the inner headers. It validates that `LC_MAIN` points into file-backed executable `__TEXT`. A universal ARM64e slice is identified by its subtype. The inspector does not map guest memory or load dependencies.

```sh
cargo run --locked -p bifrost-cli -- inspect tests/fixtures/macho/universal.macho
```

The fixture reports x86-64 entry `0x100000268` and ARM64 entry `0x100000278`. The inspector requires `LC_MAIN` for entry reporting. Legacy thread-state entry commands, 32-bit executable slices, and ARM64e entry interpretation are outside this profile. The universal fixture combines the self-authored thin fixtures at file offsets `0x4000` and `0x8000` under a big-endian fat32 header. [Apple fat directory format](https://github.com/apple-oss-distributions/xnu/blob/main/EXTERNAL_HEADERS/mach-o/fat.h) · [Apple Mach-O load commands](https://github.com/apple-oss-distributions/xnu/blob/main/EXTERNAL_HEADERS/mach-o/loader.h)

## Standalone Darwin guests

Bifröst also runs a narrow, fixed-address x86-64 Mach-O fixture with an `LC_UNIXTHREAD` entry. The loader maps checked segments with separate writable and executable permissions and initializes an empty guarded startup stack. The portable Darwin provider handles only `write` to the supplied stdout or stderr and `exit`. The loader rejects dyld and other load commands outside this profile, including dependency imports and fixups. [Mach-O load commands](https://github.com/apple-oss-distributions/xnu/blob/main/EXTERNAL_HEADERS/mach-o/loader.h) · [Darwin syscall definitions](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/syscalls.master)

```sh
cargo run --locked -p bifrost-cli -- run tests/fixtures/macho/standalone.macho
```

The fixture prints `Hello from Bifrost Darwin!` and exits with status 45. To rebuild it, assemble `standalone.S` for x86-64 with Clang, extract its `.text` to a raw binary with `llvm-objcopy --only-section=.text -O binary`, then pass that raw input and `standalone.macho` as the two arguments to `make-standalone.py`. Ordinary dyld-linked macOS executables and Apple framework compatibility remain separate work.

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
| macOS guest execution | The fixed-address x86-64 console fixture runs with two portable Darwin syscalls. Dyld, ARM64 guest startup, and framework compatibility remain open. |
| ARM64e | Establish pointer authentication and ABI behavior against native fixtures. |
| Advanced Linux userspace | Static PIE and relative relocations are tested. Guest linking, shared libraries and TLS need separate fixtures. See the [Linux profile](linux-elf.md). |
| x86-32 or RISC-V and additional backends | Establish a declared instruction subset and a deterministic differential corpus. |
| Android hosts | Check target page sizes, executable memory policy, and the interpreter on an actual device. |
| Dynamic plugins | Define a versioned interface, lifetime rules, and dependency policy. |
| Persistent cache | Verify key completeness and stale-code rejection across engine versions, host features, and process restarts. |
| Native execution | Define checked transitions and state transfer, and compare results and faults with the interpreter. |
