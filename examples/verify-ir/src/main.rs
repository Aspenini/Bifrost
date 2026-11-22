use bifrost_memory::GuestMemoryRead;
use bifrost_testkit::{FIXTURE_DATA, scalar_ir, scalar_memory};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let memory = scalar_memory()?;
    let mut bytes = [0; 8];
    memory.read(FIXTURE_DATA, &mut bytes)?;
    println!(
        "Guest input at {:#x}: {}",
        FIXTURE_DATA.0,
        u64::from_le_bytes(bytes)
    );
    let verified = scalar_ir().verify()?;
    print!("{verified}");
    Ok(())
}
