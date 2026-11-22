"""Assemble a Bifrost SDK archive from an existing release build."""

import argparse
import gzip
import hashlib
from pathlib import Path
import tarfile
import tempfile


ROOT = Path(__file__).resolve().parents[1]
LIBRARIES = {
    "pc-windows-msvc": (
        ("bin/bifrost.exe", "bifrost.exe"),
        ("bin/bifrost_ffi.dll", "bifrost_ffi.dll"),
        ("lib/bifrost_ffi.dll.lib", "bifrost_ffi.dll.lib"),
        ("lib/bifrost_ffi.lib", "bifrost_ffi.lib"),
    ),
    "unknown-linux-gnu": (
        ("bin/bifrost", "bifrost"),
        ("lib/libbifrost_ffi.so", "libbifrost_ffi.so"),
        ("lib/libbifrost_ffi.a", "libbifrost_ffi.a"),
    ),
    "apple-darwin": (
        ("bin/bifrost", "bifrost"),
        ("lib/libbifrost_ffi.dylib", "libbifrost_ffi.dylib"),
        ("lib/libbifrost_ffi.a", "libbifrost_ffi.a"),
    ),
}
TARGETS = {
    f"{arch}-{platform}"
    for arch in ("x86_64", "aarch64")
    for platform in LIBRARIES
}
SOURCES = (
    ("include/bifrost.h", "crates/bifrost-ffi/include/bifrost.h"),
    ("example/main.c", "examples/c-embed/main.c"),
    ("example/raw.c", "examples/c-embed/raw.c"),
    ("example/hello.elf", "tests/fixtures/linux/hello.elf"),
    ("LICENSE-MIT", "LICENSE-MIT"),
    ("LICENSE-APACHE", "LICENSE-APACHE"),
    ("sdk.md", "docs/sdk.md"),
)


def package(target, build, output):
    platform = target.split("-", 1)[1]
    files = [(name, build / source) for name, source in LIBRARIES[platform]]
    files.extend((name, ROOT / source) for name, source in SOURCES)
    for _, source in files:
        if not source.is_file():
            raise FileNotFoundError(f"missing SDK input: {source}")

    output.mkdir(parents=True, exist_ok=True)
    destination = output / f"bifrost-{target}.tar.gz"
    with tempfile.TemporaryDirectory(dir=output) as staging:
        temporary = Path(staging) / destination.name
        with temporary.open("wb") as compressed:
            with gzip.GzipFile(filename="", fileobj=compressed, mode="wb", mtime=0) as stream:
                with tarfile.open(fileobj=stream, mode="w|") as archive:
                    for name, source in sorted(files):
                        with source.open("rb") as data:
                            info = tarfile.TarInfo(name)
                            info.size = source.stat().st_size
                            info.mode = 0o755 if name.startswith("bin/") else 0o644
                            archive.addfile(info, data)
        temporary.replace(destination)
    return destination


def write_checksums(output):
    names = [f"bifrost-{target}.tar.gz" for target in sorted(TARGETS)]
    lines = []
    for name in names:
        with (output / name).open("rb") as archive:
            digest = hashlib.file_digest(archive, "sha256").hexdigest()
        lines.append(f"{digest}  {name}\n")

    destination = output / "SHA256SUMS"
    with tempfile.TemporaryDirectory(dir=output) as staging:
        temporary = Path(staging) / destination.name
        temporary.write_text("".join(lines), encoding="ascii", newline="\n")
        temporary.replace(destination)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", nargs="?", metavar="TARGET")
    parser.add_argument("--checksums", action="store_true", help="require and hash all six SDK archives")
    parser.add_argument("--build-dir", type=Path, default=ROOT / "target/release")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "target/sdk")
    args = parser.parse_args()
    if args.checksums:
        if args.target is not None:
            parser.error("pass a target or --checksums")
    elif args.target not in TARGETS:
        parser.error(f"select a supported target: {', '.join(sorted(TARGETS))}")
    try:
        if args.checksums:
            print(write_checksums(args.output_dir))
        else:
            print(package(args.target, args.build_dir, args.output_dir))
    except OSError as error:
        parser.exit(1, f"{parser.prog}: {error}\n")


if __name__ == "__main__":
    main()
