use bifrost_runtime::{
    RuntimeProfile,
    core::{CapabilityProvider, ExecMode, TraceSink},
    process::{self, ApiPolicy, Io, RunOptions, Vfs, WindowsIo},
};
mod trace;
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    path::Path,
};

const USAGE: &str = "Usage: bifrost doctor | version [--verbose] | capabilities [--json] | inspect FILE | trace FILE --output FILE | run [--exec auto|interp|jit] [--api auto|portable|native] [--max-steps N] [--mount /guest=HOST] FILE [-- ARGS...]";
const MAX_FILE: u64 = 64 * 1024 * 1024;
type Error = Box<dyn std::error::Error>;

fn main() {
    match command(std::env::args_os().skip(1).collect()) {
        Ok(status) => std::process::exit(status as i32),
        Err(error) => {
            eprintln!("bifrost: {error}");
            std::process::exit(1);
        }
    }
}

fn command(args: Vec<OsString>) -> Result<u32, Error> {
    match args.as_slice() {
        [] => println!("{USAGE}"),
        [flag] if flag == "--help" || flag == "-h" => println!("{USAGE}"),
        [command] if command == "capabilities" => capabilities(),
        [command] if command == "doctor" => doctor(),
        [command] if command == "version" || command == "--version" => version(false),
        [command, flag] if command == "version" && flag == "--verbose" => version(true),
        [command, flag] if command == "capabilities" && flag == "--json" => capabilities_json(),
        [command, path] if command == "inspect" => {
            let image = read_file(Path::new(path))?;
            let metadata = process::inspect(&image)?;
            println!(
                "{:?} {:?} entry {:#x}",
                metadata.format, metadata.isa, metadata.entry.0
            );
            if image.starts_with(b"MZ") {
                for import in process::inspect_pe(&image)?.imports {
                    println!("  {}!{}", import.module, import.symbol);
                }
            }
        }
        [command, rest @ ..] if command == "run" => return run(rest, &mut ()),
        [command, path, flag, output] if command == "trace" && flag == "--output" => {
            let mut trace = trace::JsonLines::open(Path::new(output))?;
            let outcome = run(&["--".into(), path.clone()], &mut trace);
            trace.finish(&outcome)?;
            return outcome;
        }
        _ => return Err(USAGE.into()),
    }
    Ok(0)
}

fn doctor() {
    let profile = RuntimeProfile::default();
    let host = profile.host();
    println!("Host {:?}/{:?}", host.os, host.isa);
    match host.page_size {
        Some(bytes) => println!("Host page size {bytes} bytes"),
        None => println!("Host page size unavailable"),
    }
    println!("Executable memory {:?}", host.executable_memory);
    for id in [
        "interpreter",
        "jit-cranelift",
        "linux-elf",
        "windows-pe",
        "macho-inspect",
    ] {
        let item = profile
            .capabilities()
            .get(id)
            .expect("diagnostic capability");
        println!("{id} {:?}  {}", item.status(), item.detail());
    }
}

fn version(verbose: bool) {
    println!("bifrost {}", env!("CARGO_PKG_VERSION"));
    if verbose {
        println!("Target {}/{}", std::env::consts::OS, std::env::consts::ARCH);
        println!(
            "Revision {}",
            option_env!("BIFROST_BUILD_REVISION").unwrap_or("unknown")
        );
        println!("Cranelift feature {}", cfg!(feature = "jit"));
    }
}

fn capabilities() {
    let profile = RuntimeProfile::default();
    let host = profile.host();
    println!("Host {:?}/{:?}", host.os, host.isa);
    println!(
        "{:<20} {:<9} {:<12} {:<10} {:<10} Support",
        "Component", "Compiled", "Implemented", "Available", "Validated"
    );
    for item in profile.capabilities().iter() {
        println!(
            "{:<20} {:<9} {:<12} {:<10} {:<10} {:?}  {}",
            item.id(),
            item.compiled(),
            item.implemented_on_any_host(),
            item.host_available(),
            item.validated(),
            item.level(),
            item.detail()
        );
    }
}

fn capabilities_json() {
    let profile = RuntimeProfile::default();
    let host = profile.host();
    println!(
        "{{\"schema\":1,\"host\":{{\"os\":\"{:?}\",\"isa\":\"{:?}\",\"executable_memory\":\"{:?}\",\"page_size\":{}}},\"capabilities\":[",
        host.os,
        host.isa,
        host.executable_memory,
        host.page_size
            .map_or_else(|| "null".into(), |size| size.to_string())
    );
    for (index, item) in profile.capabilities().iter().enumerate() {
        if index != 0 {
            println!(",");
        }
        print!(
            "{{\"id\":\"{}\",\"compiled\":{},\"implemented\":{},\"available\":{},\"validated\":{},\"support\":\"{:?}\"}}",
            item.id(),
            item.compiled(),
            item.implemented_on_any_host(),
            item.host_available(),
            item.validated(),
            item.level()
        );
    }
    println!("]}}");
}

fn read_file(path: &Path) -> Result<Vec<u8>, Error> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("file exceeds the 64 MiB input limit".into());
    }
    Ok(bytes)
}

fn run(args: &[OsString], trace: &mut dyn TraceSink) -> Result<u32, Error> {
    let mut options = RunOptions::default();
    let mut vfs = Vfs::default();
    let mut mounted = false;
    let mut api = None;
    let mut args = args.iter();
    let path = loop {
        let next = args.next().ok_or(USAGE)?;
        if next == "--exec" {
            options.execution = match args.next().and_then(|word| word.to_str()) {
                Some("auto") => ExecMode::Auto,
                Some("interp") => ExecMode::Interpreter,
                Some("jit") => ExecMode::Jit,
                _ => return Err("expected --exec auto, interp or jit".into()),
            };
        } else if next == "--max-steps" {
            options.instruction_limit = args
                .next()
                .and_then(|word| word.to_str())
                .and_then(|word| word.parse().ok())
                .filter(|limit| *limit > 0)
                .ok_or("expected a positive instruction limit")?;
        } else if next == "--api" {
            api = Some(match args.next().and_then(|word| word.to_str()) {
                Some("auto") => ApiPolicy::Auto,
                Some("portable") => ApiPolicy::Portable,
                Some("native") => ApiPolicy::Native,
                _ => return Err("expected --api auto, portable or native".into()),
            });
        } else if next == "--mount" {
            let (guest, host) = args
                .next()
                .and_then(|word| word.to_str())
                .and_then(|word| word.split_once('='))
                .ok_or("expected --mount /guest=HOST")?;
            vfs.mount(guest.as_bytes(), read_file(Path::new(host))?)?;
            mounted = true;
        } else if next == "--" {
            break args.next().ok_or(USAGE)?;
        } else if next.to_string_lossy().starts_with('-') {
            return Err(USAGE.into());
        } else {
            break next;
        }
    };
    let mut guest_args = vec![path.to_string_lossy()];
    if let Some(separator) = args.next() {
        if separator != "--" {
            return Err(USAGE.into());
        }
        guest_args.extend(args.map(|value| value.to_string_lossy()));
    }
    let argv: Vec<_> = guest_args.iter().map(|arg| arg.as_bytes()).collect();
    let image = read_file(Path::new(path))?;
    let (mut stdout, mut stderr) = (io::stdout().lock(), io::stderr().lock());
    let status = if image.starts_with(bifrost_runtime::macho::MAGIC) {
        return Err(
            "Mach-O support is limited to inspection; macOS guest execution is unavailable".into(),
        );
    } else if image.starts_with(b"MZ") {
        if mounted || argv.len() > 1 {
            return Err("PE execution currently accepts neither mounts nor guest arguments".into());
        }
        process::run_windows_traced(
            &image,
            WindowsIo {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
            api.unwrap_or(ApiPolicy::Auto),
            options,
            trace,
        )
    } else {
        if api.is_some() {
            return Err("--api applies to Windows PE images".into());
        }
        process::run_linux_traced(
            &image,
            &argv,
            &[],
            Io {
                input: &[],
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
            &vfs,
            options,
            trace,
        )
    };
    stdout.flush()?;
    stderr.flush()?;
    Ok(status?)
}
