//! Experimental metadata inspection for thin little-endian Mach-O executables.
use bifrost_core::{GuestAddress, GuestIsa};
use bifrost_loader::{Format, LoadError, MAX_IMAGE_BYTES, Metadata, slice, u32_at, u64_at};

pub const MAGIC: &[u8; 4] = b"\xcf\xfa\xed\xfe";
const MAX_COMMANDS: u32 = 4096;
const HEADER_SIZE: u64 = 32;

/// Reads the entry address from LC_MAIN and its file-backed __TEXT segment.
pub fn inspect(bytes: &[u8]) -> Result<Metadata, LoadError> {
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(LoadError::Unsupported("Mach-O input size"));
    }
    let header = slice(bytes, 0, HEADER_SIZE)?;
    if &header[..4] != MAGIC || u32_at(header, 12)? != 2 {
        return Err(LoadError::Unsupported(
            "requires a thin Mach-O 64-bit executable",
        ));
    }
    let isa = match u32_at(header, 4)? {
        0x0100_0007 => GuestIsa::X86_64,
        0x0100_000c if u32_at(header, 8)? & 0x00ff_ffff == 2 => {
            return Err(LoadError::Unsupported("Mach-O ARM64e"));
        }
        0x0100_000c => GuestIsa::AArch64,
        _ => return Err(LoadError::Unsupported("Mach-O CPU")),
    };
    let count = u32_at(header, 16)?;
    if !(1..=MAX_COMMANDS).contains(&count) {
        return Err(LoadError::Invalid("Mach-O load command count"));
    }
    let command_size = u64::from(u32_at(header, 20)?);
    let commands = slice(bytes, HEADER_SIZE, command_size)?;
    let mut cursor = 0;
    let mut entry = None;
    let mut text = None;
    for _ in 0..count {
        let kind = u32_at(commands, cursor)?;
        let size = u64::from(u32_at(commands, cursor + 4)?);
        if size < 8 || !size.is_multiple_of(8) {
            return Err(LoadError::Invalid("Mach-O load command size"));
        }
        let command = slice(commands, cursor, size)?;
        match kind {
            0x19 => {
                let segment = inspect_segment(bytes, command)?;
                if &command[8..24] == b"__TEXT\0\0\0\0\0\0\0\0\0\0"
                    && text.replace(segment).is_some()
                {
                    return Err(LoadError::Invalid("duplicate Mach-O __TEXT"));
                }
            }
            0x8000_0028 if size == 24 && entry.is_none() => entry = Some(u64_at(command, 8)?),
            0x8000_0028 => return Err(LoadError::Invalid("Mach-O LC_MAIN")),
            _ => {}
        }
        cursor += size;
    }
    if cursor != command_size {
        return Err(LoadError::Invalid("Mach-O command table length"));
    }
    let entry = entry.ok_or(LoadError::Unsupported("Mach-O LC_MAIN required"))?;
    let text = text.ok_or(LoadError::Invalid("Mach-O __TEXT required"))?;
    let address = text
        .address
        .checked_add(entry)
        .filter(|_| {
            text.offset == 0
                && text.executable
                && (HEADER_SIZE + command_size..text.size).contains(&entry)
        })
        .ok_or(LoadError::Invalid("Mach-O entry range"))?;
    Ok(Metadata {
        format: Format::MachO,
        isa,
        entry: GuestAddress(address),
    })
}

struct Segment {
    address: u64,
    offset: u64,
    size: u64,
    executable: bool,
}

fn inspect_segment(bytes: &[u8], command: &[u8]) -> Result<Segment, LoadError> {
    let sections = u64::from(u32_at(command, 64)?);
    if command.len() as u64 != 72 + sections * 80 {
        return Err(LoadError::Invalid("Mach-O section table length"));
    }
    let address = u64_at(command, 24)?;
    let virtual_size = u64_at(command, 32)?;
    let offset = u64_at(command, 40)?;
    let size = u64_at(command, 48)?;
    let max_protection = u32_at(command, 56)?;
    let protection = u32_at(command, 60)?;
    if address.checked_add(virtual_size).is_none()
        || size > virtual_size
        || max_protection & !7 != 0
        || protection & !max_protection != 0
    {
        return Err(LoadError::Invalid("Mach-O segment bounds or protection"));
    }
    if size != 0 {
        slice(bytes, offset, size)?;
    }
    Ok(Segment {
        address,
        offset,
        size,
        executable: protection & 4 != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const X86: &[u8] = include_bytes!("../../../tests/fixtures/macho/entry-x86_64.macho");
    const ARM: &[u8] = include_bytes!("../../../tests/fixtures/macho/entry-arm64.macho");

    #[test]
    fn reads_the_linkers_entry_on_both_guest_architectures() {
        for (image, isa, entry) in [
            (X86, GuestIsa::X86_64, 0x1_0000_0268),
            (ARM, GuestIsa::AArch64, 0x1_0000_0278),
        ] {
            assert_eq!(
                inspect(image),
                Ok(Metadata {
                    format: Format::MachO,
                    isa,
                    entry: GuestAddress(entry),
                })
            );
        }
    }

    #[test]
    fn rejects_truncation_and_malformed_load_commands() {
        for length in [0, 31, 32, 100, X86.len() - 1] {
            assert!(inspect(&X86[..length]).is_err());
        }
        let mut invalid = X86.to_vec();
        invalid[36..40].copy_from_slice(&0_u32.to_le_bytes());
        assert!(matches!(inspect(&invalid), Err(LoadError::Invalid(_))));
    }

    #[test]
    fn distinguishes_unsupported_arm64e() {
        let mut image = ARM.to_vec();
        image[8..12].copy_from_slice(&2_u32.to_le_bytes());
        assert_eq!(
            inspect(&image),
            Err(LoadError::Unsupported("Mach-O ARM64e"))
        );
    }
}
