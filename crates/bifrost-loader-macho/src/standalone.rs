use super::{HEADER_SIZE, MAGIC, MAX_COMMANDS, inspect_segment};
use bifrost_core::{GuestAddress, GuestIsa, Protection};
use bifrost_loader::{
    Format, Image, LoadError, MAX_IMAGE_BYTES, Metadata, PAGE, Segment, slice, u32_at, u64_at,
};

/// Accepts a fixed-address x86-64 Mach-O with a UNIX thread entry and no dyld commands.
pub fn standalone_image(bytes: &[u8]) -> Result<Image<'_>, LoadError> {
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(LoadError::Unsupported("Mach-O input size"));
    }
    let header = slice(bytes, 0, HEADER_SIZE)?;
    if &header[..4] != MAGIC || u32_at(header, 4)? != 0x0100_0007 || u32_at(header, 12)? != 2 {
        return Err(LoadError::Unsupported(
            "requires a thin x86-64 Mach-O executable",
        ));
    }
    // Only MH_NOUNDEFS is accepted. Dyld-linked, PIE and executable-stack flags are outside this profile.
    if u32_at(header, 24)? & !1 != 0 || u32_at(header, 28)? != 0 {
        return Err(LoadError::Unsupported(
            "Mach-O flags require a fixed standalone image",
        ));
    }
    let count = u32_at(header, 16)?;
    if !(1..=MAX_COMMANDS).contains(&count) {
        return Err(LoadError::Invalid("Mach-O load command count"));
    }
    let commands = slice(bytes, HEADER_SIZE, u64::from(u32_at(header, 20)?))?;
    let mut cursor = 0;
    let mut entry = None;
    let mut segments = Vec::new();
    for _ in 0..count {
        let size = u64::from(u32_at(commands, cursor + 4)?);
        if size < 8 || !size.is_multiple_of(8) {
            return Err(LoadError::Invalid("Mach-O load command size"));
        }
        let command = slice(commands, cursor, size)?;
        match u32_at(command, 0)? {
            0x19 => {
                let segment = inspect_segment(bytes, command)?;
                let memory_size = u64_at(command, 32)?;
                let init = u32_at(command, 60)?;
                if segment.address == 0
                    && init == 0
                    && u32_at(command, 56)? == 0
                    && segment.size == 0
                    && &command[8..24] == b"__PAGEZERO\0\0\0\0\0\0"
                {
                    cursor += size;
                    continue;
                }
                let protection = match init {
                    1 => Protection::Read,
                    3 => Protection::ReadWrite,
                    5 => Protection::ReadExecute,
                    _ => {
                        return Err(LoadError::Unsupported(
                            "Mach-O standalone segment protection",
                        ));
                    }
                };
                if !segment.address.is_multiple_of(PAGE) || !segment.offset.is_multiple_of(PAGE) {
                    return Err(LoadError::Unsupported(
                        "Mach-O standalone segment alignment",
                    ));
                }
                let data = slice(bytes, segment.offset, segment.size)?;
                segments.push(Segment {
                    address: GuestAddress(segment.address),
                    size: memory_size,
                    data,
                    protection,
                });
            }
            5 if entry.is_none()
                && size == 184
                && u32_at(command, 8)? == 4
                && u32_at(command, 12)? == 42 =>
            {
                if command[16..144]
                    .iter()
                    .chain(command[152..].iter())
                    .any(|&byte| byte != 0)
                {
                    return Err(LoadError::Unsupported(
                        "Mach-O standalone thread state must specify only RIP",
                    ));
                }
                entry = Some(GuestAddress(u64_at(command, 144)?));
            }
            5 => return Err(LoadError::Invalid("Mach-O UNIX thread state")),
            _ => {
                return Err(LoadError::Unsupported(
                    "Mach-O standalone load command or dyld dependency",
                ));
            }
        }
        cursor += size;
    }
    if cursor != commands.len() as u64 {
        return Err(LoadError::Invalid("Mach-O command table length"));
    }
    Ok(Image {
        metadata: Metadata {
            format: Format::MachO,
            isa: GuestIsa::X86_64,
            entry: entry.ok_or(LoadError::Unsupported(
                "Mach-O standalone UNIX thread entry required",
            ))?,
        },
        segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/macho/standalone.macho");

    #[test]
    fn maps_only_fixed_image_segments_and_executable_entry() {
        let image = standalone_image(FIXTURE).unwrap();
        assert_eq!(image.metadata.entry, GuestAddress(0x1_0000_0200));
        assert!(image.map().is_ok());
        let mut malformed = FIXTURE.to_vec();
        // Move RIP past the file-backed text.
        malformed[248..256].copy_from_slice(&0x1_0000_1000_u64.to_le_bytes());
        assert!(standalone_image(&malformed).unwrap().map().is_err());
    }

    #[test]
    fn rejects_nonzero_startup_registers_and_unknown_load_commands() {
        let mut malformed = FIXTURE.to_vec();
        malformed[120] = 1;
        assert!(matches!(
            standalone_image(&malformed),
            Err(LoadError::Unsupported(_))
        ));
        malformed[120] = 0;
        malformed[104..108].copy_from_slice(&0x0e_u32.to_le_bytes());
        assert!(matches!(
            standalone_image(&malformed),
            Err(LoadError::Unsupported(_))
        ));
    }
}
