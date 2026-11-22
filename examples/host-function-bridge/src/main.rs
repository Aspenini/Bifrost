use bifrost_example_host_function_bridge::{Backend, GUEST_ABIS, JIT_BUILT, demo};
use bifrost_runtime::abi::AbiDescriptor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = AbiDescriptor::native().ok_or("native host ABI unavailable")?;
    let backend = if cfg!(feature = "jit-cranelift") {
        if !JIT_BUILT {
            return Err("system JIT unavailable on this host".into());
        }
        Backend::Jit
    } else {
        Backend::Interpreter
    };
    println!("Host {:?}, backend {backend:?}", host.id());
    for guest in GUEST_ABIS {
        let result = demo(guest, backend)?;
        println!(
            "{guest:?}: scalar={} stack={} signed={} mixed={} pair=({}, {}) buffer={:?} handle={} callback={}",
            result.scalar,
            result.stack_scalar,
            result.signed,
            result.mixed,
            result.aggregate.first,
            result.aggregate.second,
            result.buffer,
            result.handle,
            result.callback
        );
    }
    Ok(())
}
