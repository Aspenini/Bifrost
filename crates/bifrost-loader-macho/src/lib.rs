//! Experimental metadata inspection for thin and universal Mach-O executables.
use bifrost_core::{GuestAddress, GuestIsa};
use bifrost_loader::{Format, LoadError, MAX_IMAGE_BYTES, Metadata, slice, u32_at, u64_at};

pub const MAGIC: &[u8; 4] = b"\xcf\xfa\xed\xfe";
const FAT32: &[u8; 4] = b"\xca\xfe\xba\xbe";
const FAT64: &[u8; 4] = b"\xca\xfe\xba\xbf";
const MAX_COMMANDS: u32 = 4096;
const HEADER_SIZE: u64 = 32;

pub fn is_universal(bytes: &[u8]) -> bool {
    bytes.starts_with(FAT32) || bytes.starts_with(FAT64)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UniversalSlice {
    pub offset: u64,
    pub size: u64,
    pub architecture: SliceArchitecture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SliceArchitecture {
    Executable(Metadata),
    Arm64e { subtype: u32 },
}

/// Validates the fat directory and checks each supported slice against its inner header.
pub fn inspect_universal(bytes: &[u8]) -> Result<Vec<UniversalSlice>, LoadError> {
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(LoadError::Unsupported("Mach-O input size"));
    }
    let wide = match slice(bytes, 0, 4)? {
        magic if magic == FAT32 => false,
        magic if magic == FAT64 => true,
        _ => return Err(LoadError::Unsupported("requires Mach-O fat container")),
    };
    let count = u64::from(be32(bytes, 4)?);
    if !(1..=32).contains(&count) {
        return Err(LoadError::Invalid("Mach-O fat slice count"));
    }
    let width = if wide { 32 } else { 20 };
    let directory_end = 8 + count * width;
    let table = slice(bytes, 8, count * width)?;
    let mut entries = Vec::new();
    let mut identities = std::collections::BTreeSet::new();
    for row in table.chunks_exact(width as usize) {
        let cpu = be32(row, 0)?;
        let subtype = be32(row, 4)?;
        if !identities.insert((cpu, subtype)) {
            return Err(LoadError::Invalid("duplicate Mach-O fat architecture"));
        }
        let (offset, size, alignment) = if wide {
            if be32(row, 28)? != 0 {
                return Err(LoadError::Invalid("Mach-O fat64 reserved field"));
            }
            (be64(row, 8)?, be64(row, 16)?, be32(row, 24)?)
        } else {
            (
                u64::from(be32(row, 8)?),
                u64::from(be32(row, 12)?),
                be32(row, 16)?,
            )
        };
        if alignment > 31 || offset < directory_end || offset % (1_u64 << alignment) != 0 {
            return Err(LoadError::Invalid("Mach-O fat slice alignment or offset"));
        }
        let data = slice(bytes, offset, size)?;
        let header = slice(data, 0, HEADER_SIZE)?;
        if &header[..4] != MAGIC || u32_at(header, 4)? != cpu || u32_at(header, 8)? != subtype {
            return Err(LoadError::Invalid(
                "Mach-O fat and inner slice headers disagree",
            ));
        }
        entries.push((offset, size, cpu, subtype, data));
    }
    let mut ranges = entries
        .iter()
        .map(|&(offset, size, ..)| (offset, offset + size))
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(LoadError::Invalid("overlapping Mach-O fat slices"));
    }
    entries
        .into_iter()
        .map(|(offset, size, cpu, subtype, data)| {
            let architecture = if cpu == 0x0100_000c && subtype & 0x00ff_ffff == 2 {
                if u32_at(data, 12)? != 2 {
                    return Err(LoadError::Unsupported(
                        "requires an ARM64e Mach-O executable",
                    ));
                }
                SliceArchitecture::Arm64e { subtype }
            } else {
                SliceArchitecture::Executable(inspect(data)?)
            };
            Ok(UniversalSlice {
                offset,
                size,
                architecture,
            })
        })
        .collect()
}

fn be32(bytes: &[u8], offset: u64) -> Result<u32, LoadError> {
    Ok(u32::from_be_bytes(
        slice(bytes, offset, 4)?.try_into().unwrap(),
    ))
}

fn be64(bytes: &[u8], offset: u64) -> Result<u64, LoadError> {
    Ok(u64::from_be_bytes(
        slice(bytes, offset, 8)?.try_into().unwrap(),
    ))
}

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
    const UNIVERSAL: &[u8] = include_bytes!("../../../tests/fixtures/macho/universal.macho");

    #[test]
    fn reads_fat32_and_fat64_with_consistent_slice_metadata() {
        let slices = inspect_universal(UNIVERSAL).unwrap();
        assert_eq!(slices.len(), 2);
        for (slice, inner) in slices.iter().zip([X86, ARM]) {
            assert_eq!(slice.size, inner.len() as u64);
            assert_eq!(
                slice.architecture,
                SliceArchitecture::Executable(inspect(inner).unwrap())
            );
        }
        let mut wide = UNIVERSAL.to_vec();
        wide[..4].copy_from_slice(FAT64);
        let mut table = Vec::new();
        for (slice, inner) in slices.iter().zip([X86, ARM]) {
            table.extend(u32_at(inner, 4).unwrap().to_be_bytes());
            table.extend(u32_at(inner, 8).unwrap().to_be_bytes());
            table.extend(slice.offset.to_be_bytes());
            table.extend(slice.size.to_be_bytes());
            table.extend(14_u32.to_be_bytes());
            table.extend(0_u32.to_be_bytes());
        }
        wide[8..8 + table.len()].copy_from_slice(&table);
        assert_eq!(inspect_universal(&wide).unwrap(), slices);
        wide[39] = 1;
        assert!(matches!(
            inspect_universal(&wide),
            Err(LoadError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_overlap_mismatch_and_truncation() {
        let mut image = UNIVERSAL.to_vec();
        image[20..24].copy_from_slice(&0x4001_u32.to_be_bytes());
        assert_eq!(
            inspect_universal(&image),
            Err(LoadError::Invalid("overlapping Mach-O fat slices"))
        );
        let mut image = UNIVERSAL.to_vec();
        image[0x8008..0x800c].copy_from_slice(&7_u32.to_le_bytes());
        assert_eq!(
            inspect_universal(&image),
            Err(LoadError::Invalid(
                "Mach-O fat and inner slice headers disagree"
            ))
        );
        assert!(inspect_universal(&UNIVERSAL[..UNIVERSAL.len() - 1]).is_err());
    }

    #[test]
    fn identifies_arm64e_without_claiming_support_for_its_entry_point() {
        let mut image = UNIVERSAL.to_vec();
        image[32..36].copy_from_slice(&2_u32.to_be_bytes());
        image[0x8008..0x800c].copy_from_slice(&2_u32.to_le_bytes());
        assert_eq!(
            inspect_universal(&image).unwrap()[1].architecture,
            SliceArchitecture::Arm64e { subtype: 2 }
        );
    }

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
