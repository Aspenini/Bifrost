use bifrost_runtime::{
    RuntimeProfile,
    core::{CapabilityProvider, ExecMode},
    process::{self, Io, RunOptions, Vfs},
};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    path::Path,
    process::ExitCode,
};

const USAGE: &str = "Usage: bifrost capabilities | inspect FILE | run [--exec auto|interp|jit] [--mount /guest=HOST] FILE [-- ARGS...]";
const MAX_FILE: u64 = 64 * 1024 * 1024;
type Error = Box<dyn std::error::Error>;

fn main() -> ExitCode {
    match command(std::env::args_os().skip(1).collect()) {
        Ok(status) => ExitCode::from(status as u8),
        Err(error) => {
            eprintln!("bifrost: {error}");
            ExitCode::from(1)
        }
    }
}

fn command(args: Vec<OsString>) -> Result<u32, Error> {
    match args.as_slice() {
        [] => println!("{USAGE}"),
        [flag] if flag == "--help" || flag == "-h" => println!("{USAGE}"),
        [command] if command == "capabilities" => capabilities(),
        [command, path] if command == "inspect" => {
            let metadata = process::inspect(&read_file(Path::new(path))?)?;
            println!(
                "{:?} {:?} entry {:#x}",
                metadata.format, metadata.isa, metadata.entry.0
            );
        }
        [command, rest @ ..] if command == "run" => return run(rest),
        _ => return Err(USAGE.into()),
    }
    Ok(0)
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

fn read_file(path: &Path) -> Result<Vec<u8>, Error> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("file exceeds the 64 MiB input limit".into());
    }
    Ok(bytes)
}

fn run(args: &[OsString]) -> Result<u32, Error> {
    let mut options = RunOptions::default();
    let mut vfs = Vfs::default();
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
        } else if next == "--mount" {
            let (guest, host) = args
                .next()
                .and_then(|word| word.to_str())
                .and_then(|word| word.split_once('='))
                .ok_or("expected --mount /guest=HOST")?;
            vfs.mount(guest.as_bytes(), read_file(Path::new(host))?)?;
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
    let status = process::run_linux(
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
    );
    stdout.flush()?;
    stderr.flush()?;
    Ok(status?)
}
