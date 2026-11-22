use bifrost_runtime::{RuntimeProfile, core::CapabilityProvider};
use std::process::ExitCode;

const USAGE: &str = "Usage: bifrost capabilities";

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [] => println!("{USAGE}"),
        [flag] if flag == "--help" || flag == "-h" => println!("{USAGE}"),
        [command] if command == "capabilities" => {
            let profile = RuntimeProfile::default();
            let host = profile.host();
            println!("Host {:?}/{:?}", host.os, host.isa);
            println!(
                "{:<20} {:<9} {:<12} {:<10} {:<10} Support",
                "Component", "Compiled", "Implemented", "Available", "Validated"
            );
            for capability in profile.capabilities().iter() {
                println!(
                    "{:<20} {:<9} {:<12} {:<10} {:<10} {:?}  {}",
                    capability.id(),
                    capability.compiled(),
                    capability.implemented_on_any_host(),
                    capability.host_available(),
                    capability.validated(),
                    capability.level(),
                    capability.detail()
                );
            }
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}
