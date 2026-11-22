# Bifröst C SDK

The SDK ships a C header, a shared library, a static library, and the CLI. Check `bifrost_abi_version()` before using the interface. ABI 1 accepts static Linux ELF and fixed-base Windows PE images through the interpreter with the portable OS providers. Each run creates an independent guest address space and initial thread.

Create a session with `bifrost_create()`, query a length-delimited capability ID, and release the session with `bifrost_destroy()`. The caller owns the image, output and result buffers. Keep each buffer valid through the call and keep writable ranges disjoint from all other arguments. Calls can share a live session with separate result and output buffers. Coordinate destruction with all callers.

The API returns a status code independent of the guest exit code. The exit code is valid for `BIFROST_OK` and `BIFROST_OUTPUT_TRUNCATED`. Standard output and standard error are combined in call order. If the buffer fills, the library discards remaining output and reports truncation after a successful guest exit. The output is length-delimited and may contain any byte. The C API provides empty guest input, no host mounts, and a limit of one million steps. The image and output ranges are each capped at 64 MiB.

Build and run the example from the repository root on Linux or macOS.

```sh
cargo build --locked -p bifrost-ffi
cc -Wall -Wextra -Werror -I crates/bifrost-ffi/include examples/c-embed/main.c \
  -L target/debug -lbifrost_ffi -Wl,-rpath,"$PWD/target/debug" -o target/debug/c-embed
target/debug/c-embed tests/fixtures/linux/hello.elf
```

Use Clang with the MSVC toolchain on Windows.

```powershell
cargo build --locked -p bifrost-ffi
clang -Wall -Wextra -Werror -I crates/bifrost-ffi/include examples/c-embed/main.c target/debug/bifrost_ffi.dll.lib -o target/debug/c-embed.exe
./target/debug/c-embed.exe tests/fixtures/linux/hello.elf
```

The SDK workflow builds and tests each archive on its matching runner. On a manual run or tag push, it also assembles the `bifrost-sdk-release` artifact after all six runners succeed.

## Prepare a release

Use a manual SDK workflow run to validate the release commit before tagging it. Push the version tag for that verified commit, then download `bifrost-sdk-release` from the tagged workflow run. The artifact contains all six native-tested SDK archives and a `SHA256SUMS` file.

Verify the downloaded files from the artifact directory. Use the existing tag to stage a GitHub release draft for review.

```sh
sha256sum --check SHA256SUMS
gh release create v0.1.0 --draft --verify-tag --generate-notes bifrost-*.tar.gz SHA256SUMS
```

Replace `v0.1.0` with the tag for the artifact you downloaded. Review the draft and its assets before publishing.

## Use an extracted SDK

From the extracted archive root on Linux or macOS, link the example against the bundled shared library.

```sh
cc -Wall -Wextra -Werror -I include example/main.c -L lib -lbifrost_ffi -Wl,-rpath,"$PWD/lib" -o example/c-embed
example/c-embed example/hello.elf
```

From the extracted archive root on Windows, place the example next to the DLL. Use the corresponding `--target` if Clang defaults to a different architecture.

```powershell
clang -Wall -Wextra -Werror -I include example/main.c lib/bifrost_ffi.dll.lib -o bin/c-embed.exe
./bin/c-embed.exe example/hello.elf
```

To assemble a local archive from the repository, build the release CLI and library, then pass the matching target triple to the packager. Use `--build-dir target/<triple>/release` for a Cargo build with an explicit `--target`.

```sh
cargo build --locked --release -p bifrost-cli -p bifrost-ffi
python scripts/package-sdk.py x86_64-unknown-linux-gnu
```

## Host isolation

For Linux hosts with Bubblewrap and enabled user namespaces, the companion launcher runs Bifröst in a separate mount, network, and PID namespace. It clears the environment, drops capabilities, exposes system libraries read-only, and supplies the runner and guest image through explicit read-only mounts. It uses the interpreter and exits if isolation setup fails.

```sh
cargo build --locked -p bifrost-cli
sh scripts/run-linux-isolated.sh target/debug/bifrost tests/fixtures/linux/hello.elf
```

The in-process C API and default CLI provide guest memory and service boundaries only. Use an external OS policy for host isolation and host resource quotas.
