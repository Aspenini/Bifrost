# Windows PE execution

Bifröst loads fixed-base x86-64 PE32+ console executables with named imports. The process starts at the PE entry point with a Windows x64 stack and an explicit set of guest handles.

```sh
cargo run --locked -p bifrost-cli -- inspect tests/fixtures/windows/hello.exe
cargo run --locked -p bifrost-cli -- run --api portable tests/fixtures/windows/hello.exe
```

The fixture writes `Hello from Bifrost PE!` with CRLF and exits with status 43. Auto and portable API policies use the output streams supplied by the caller. The native policy selects Windows host standard-output and standard-error writes. Other services stay inside the guest provider.

The provider recognizes named exports from KERNEL32 and KERNELBASE for `GetStdHandle`, `WriteFile`, `CloseHandle`, `GetLastError`, `SetLastError`, and `ExitProcess`. It also accepts `NtClose` and `NtTerminateProcess` from NTDLL. Only a PE's declared and resolved imports can be called. Unsupported imports report their module and symbol before execution.

The profile requires fixed addresses and synchronous writes to guest standard handles. Relocations, TLS, ordinal imports, DLL loading, startup environment and arguments, and overlapped I/O are outside this profile.

Rebuild either fixture with LLVM from the repository root.

```powershell
New-Item -ItemType Directory -Force target/guest-fixtures
llvm-dlltool -m i386:x86-64 -d tests/fixtures/windows/kernel32.def -l target/guest-fixtures/kernel32.lib
foreach ($name in 'hello', 'unsupported') {
    clang --target=x86_64-pc-windows-msvc -c "tests/fixtures/windows/$name.S" -o "target/guest-fixtures/$name.obj"
    lld-link /entry:entry /subsystem:console /nodefaultlib /fixed /dynamicbase:no /Brepro "/out:tests/fixtures/windows/$name.exe" "target/guest-fixtures/$name.obj" target/guest-fixtures/kernel32.lib
}
```
