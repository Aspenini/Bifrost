use std::{env, error::Error, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    if !env::args_os().skip(1).eq(["check-deps"]) {
        return Err("usage: cargo run -p xtask -- check-deps".into());
    }
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "tree",
            "--offline",
            "--locked",
            "--workspace",
            "--all-features",
            "--no-dedupe",
            "--target",
            "all",
            "--edges",
            "normal,build,dev",
            "--prefix",
            "depth",
            "--format",
            "{p}",
            "--color",
            "never",
        ])
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }

    let tree = String::from_utf8(output.stdout)?;
    let mut root = "";
    for line in tree.lines().filter(|line| !line.is_empty()) {
        let depth_len = line.bytes().take_while(u8::is_ascii_digit).count();
        let depth: usize = line[..depth_len].parse()?;
        let name = line[depth_len..]
            .split_whitespace()
            .next()
            .ok_or("missing cargo package name")?;
        if depth == 0 {
            root = name;
        }
        if depth == 1 && !allowed(root).contains(&name) {
            return Err(format!("dependency policy forbids {root} -> {name}").into());
        }
    }
    if root.is_empty() {
        return Err("cargo tree did not list workspace packages".into());
    }
    println!("Workspace dependency policy passed");
    Ok(())
}

fn allowed(package: &str) -> &'static [&'static str] {
    match package {
        "bifrost-memory" | "bifrost-executor" | "bifrost-host" | "bifrost-ir" => &["bifrost-core"],
        "bifrost-codegen" => &["bifrost-core", "bifrost-ir"],
        "bifrost-testkit" => &["bifrost-core", "bifrost-ir", "bifrost-memory"],
        "bifrost-cli" => &["bifrost-runtime"],
        "bifrost-example-verify-ir" => &["bifrost-memory", "bifrost-testkit"],
        "bifrost-runtime" => &[
            "bifrost-core",
            "bifrost-ir",
            "bifrost-memory",
            "bifrost-decoder-x86",
            "bifrost-lifter-x86",
            "bifrost-decoder-arm64",
            "bifrost-lifter-arm64",
            "bifrost-interpreter",
            "bifrost-codegen-clif",
            "bifrost-loader-elf",
            "bifrost-loader-pe",
            "bifrost-os-linux",
            "bifrost-os-windows",
            "bifrost-host-windows",
            "bifrost-host-linux",
        ],
        _ => &[],
    }
}
