//! Fixed-base PE32+ console images with explicitly resolved named imports.
use bifrost_core::{GuestAddress, GuestIsa, GuestMemory, Protection, X86State};
use bifrost_loader::{
    Format, Image, LoadError, MAX_IMAGE_BYTES, Metadata, PAGE, Segment, slice, u16_at, u32_at,
    u64_at,
};
use bifrost_memory::AddressSpace;
use std::collections::BTreeSet;

const STACK_TOP: u64 = 0x7fff_ffff_0000;
const STACK_SIZE: u64 = 64 * PAGE;
const MAX_IMPORTS: usize = 512;

#[derive(Debug)]
pub struct Import<'a> {
    pub module: &'a str,
    pub symbol: &'a str,
    slot: GuestAddress,
    protection: Protection,
}
pub struct Pe<'a> {
    pub image: Image<'a>,
    pub imports: Vec<Import<'a>>,
}

pub fn parse(bytes: &[u8]) -> Result<Pe<'_>, LoadError> {
    if slice(bytes, 0, 2)? != b"MZ" {
        return Err(LoadError::Invalid("DOS signature"));
    }
    let header = u64::from(u32_at(bytes, 0x3c)?);
    if header < 64 || slice(bytes, header, 4)? != b"PE\0\0" {
        return Err(LoadError::Invalid("PE signature"));
    }
    let coff = slice(bytes, header + 4, 20)?;
    let sections = u16_at(coff, 2)?;
    let optional_len = u64::from(u16_at(coff, 16)?);
    let flags = u16_at(coff, 18)?;
    if u16_at(coff, 0)? != 0x8664 || flags & 0x2002 != 2 {
        return Err(LoadError::Unsupported("requires an x64 PE executable"));
    }
    if !(1..=96).contains(&sections) || !(112..=512).contains(&optional_len) {
        return Err(LoadError::Invalid("PE section or optional header size"));
    }
    let optional = slice(bytes, header + 24, optional_len)?;
    if u16_at(optional, 0)? != 0x20b || u16_at(optional, 68)? != 3 {
        return Err(LoadError::Unsupported("requires PE32+ console subsystem"));
    }
    let image_base = u64_at(optional, 24)?;
    let image_size = u64::from(u32_at(optional, 56)?);
    let image_end = image_base
        .checked_add(image_size)
        .ok_or(LoadError::Invalid("PE image range"))?;
    let header_size = u64::from(u32_at(optional, 60)?);
    let file_alignment = u64::from(u32_at(optional, 36)?);
    if !image_base.is_multiple_of(65536)
        || image_base < PAGE
        || image_size > MAX_IMAGE_BYTES
        || image_end > 0x0000_8000_0000_0000
        || !image_size.is_multiple_of(PAGE)
        || u64::from(u32_at(optional, 32)?) != PAGE
        || !(512..=PAGE).contains(&file_alignment)
        || !file_alignment.is_power_of_two()
        || header_size > PAGE
        || !header_size.is_multiple_of(file_alignment)
    {
        return Err(LoadError::Unsupported("PE layout or alignment"));
    }
    let directory_count = u64::from(u32_at(optional, 108)?);
    if directory_count > 16 || 112 + directory_count * 8 > optional_len {
        return Err(LoadError::Invalid("PE data directory table"));
    }
    let directory = |index| -> Result<(u64, u64), LoadError> {
        if index >= directory_count {
            return Ok((0, 0));
        }
        Ok((
            u64::from(u32_at(optional, 112 + index * 8)?),
            u64::from(u32_at(optional, 116 + index * 8)?),
        ))
    };
    for index in [5, 9, 11, 13, 14] {
        if directory(index)? != (0, 0) {
            return Err(LoadError::Unsupported(
                "PE relocation, TLS, bound/delay import or CLR",
            ));
        }
    }
    let section_start = header + 24 + optional_len;
    let section_bytes = u64::from(sections) * 40;
    if section_start + section_bytes > header_size {
        return Err(LoadError::Invalid("PE section table"));
    }
    let mut segments = vec![Segment {
        address: GuestAddress(image_base),
        size: header_size,
        data: slice(bytes, 0, header_size)?,
        protection: Protection::Read,
    }];
    for section in slice(bytes, section_start, section_bytes)?.chunks_exact(40) {
        let rva = u64::from(u32_at(section, 12)?);
        let raw_size = u64::from(u32_at(section, 16)?);
        let file_offset = u64::from(u32_at(section, 20)?);
        let size = u64::from(u32_at(section, 8)?).max(raw_size);
        if size == 0 {
            continue;
        }
        if rva < PAGE
            || !rva.is_multiple_of(PAGE)
            || rva + size > image_size
            || (raw_size != 0
                && (file_offset < header_size || !file_offset.is_multiple_of(file_alignment)))
        {
            return Err(LoadError::Invalid("PE section range"));
        }
        let protection = match (u32_at(section, 36)? >> 29) & 7 {
            0 => Protection::None,
            1 => Protection::Execute,
            2 => Protection::Read,
            3 => Protection::ReadExecute,
            4 => Protection::Write,
            6 => Protection::ReadWrite,
            _ => return Err(LoadError::Unsupported("writable executable PE section")),
        };
        segments.push(Segment {
            address: GuestAddress(image_base + rva),
            size,
            data: slice(bytes, file_offset, raw_size)?,
            protection,
        });
    }
    let (import_rva, import_size) = directory(1)?;
    let imports = parse_imports(&segments, image_base, import_rva, import_size)?;
    let entry_rva = u64::from(u32_at(optional, 16)?);
    if entry_rva >= image_size {
        return Err(LoadError::Invalid("PE entry RVA"));
    }
    Ok(Pe {
        image: Image {
            metadata: Metadata {
                format: Format::Pe,
                isa: GuestIsa::X86_64,
                entry: GuestAddress(image_base + entry_rva),
            },
            segments,
        },
        imports,
    })
}

fn parse_imports<'a>(
    segments: &[Segment<'a>],
    base: u64,
    rva: u64,
    size: u64,
) -> Result<Vec<Import<'a>>, LoadError> {
    if (rva, size) == (0, 0) {
        return Ok(Vec::new());
    }
    if rva == 0 || !(20..=65536).contains(&size) {
        return Err(LoadError::Invalid("PE import directory"));
    }
    let (table, _) = data_at(segments, base, rva)?;
    let mut imports = Vec::new();
    let mut slots = BTreeSet::new();
    for descriptor in slice(table, 0, size)?.chunks_exact(20) {
        if descriptor.iter().all(|&byte| byte == 0) {
            return Ok(imports);
        }
        let lookup = u64::from(u32_at(descriptor, 0)?);
        let name = u64::from(u32_at(descriptor, 12)?);
        let iat = u64::from(u32_at(descriptor, 16)?);
        if name == 0 || iat == 0 || !iat.is_multiple_of(8) {
            return Err(LoadError::Invalid("PE import descriptor"));
        }
        let module = string_at(segments, base, name)?;
        let (thunks, _) = data_at(segments, base, if lookup == 0 { iat } else { lookup })?;
        for index in 0..=MAX_IMPORTS {
            let offset = (index as u64) * 8;
            let hint_name = u64_at(thunks, offset)?;
            if hint_name == 0 {
                break;
            }
            if imports.len() >= MAX_IMPORTS || index == MAX_IMPORTS {
                return Err(LoadError::Unsupported("PE import count"));
            }
            if hint_name > u64::from(u32::MAX) {
                return Err(LoadError::Unsupported("PE ordinal imports"));
            }
            let symbol = string_at(segments, base, hint_name + 2)?;
            let slot_rva = iat + offset;
            let (slot_bytes, protection) = data_at(segments, base, slot_rva)?;
            if slot_bytes.len() < 8
                || !matches!(protection, Protection::Read | Protection::ReadWrite)
                || !slots.insert(slot_rva)
            {
                return Err(LoadError::Invalid("PE import address slot"));
            }
            imports.push(Import {
                module,
                symbol,
                slot: GuestAddress(base + slot_rva),
                protection,
            });
        }
    }
    Err(LoadError::Invalid("unterminated PE import directory"))
}

fn data_at<'a>(
    segments: &[Segment<'a>],
    base: u64,
    rva: u64,
) -> Result<(&'a [u8], Protection), LoadError> {
    let address = base.checked_add(rva).ok_or(LoadError::Invalid("PE RVA"))?;
    for segment in segments {
        if let Some(offset) = address.checked_sub(segment.address.0)
            && offset < segment.data.len() as u64
        {
            return Ok((&segment.data[offset as usize..], segment.protection));
        }
    }
    Err(LoadError::Invalid("PE RVA outside file-backed sections"))
}
fn string_at<'a>(segments: &[Segment<'a>], base: u64, rva: u64) -> Result<&'a str, LoadError> {
    let (bytes, _) = data_at(segments, base, rva)?;
    let end = bytes
        .iter()
        .take(256)
        .position(|&byte| byte == 0)
        .ok_or(LoadError::Invalid("PE import name"))?;
    let name = &bytes[..end];
    if name.is_empty() || name.iter().any(|&byte| !byte.is_ascii_graphic()) {
        return Err(LoadError::Invalid("PE import name"));
    }
    std::str::from_utf8(name).map_err(|_| LoadError::Invalid("PE import name"))
}

impl Pe<'_> {
    pub fn start(
        &self,
        targets: &[GuestAddress],
        return_address: GuestAddress,
    ) -> Result<(AddressSpace, X86State), LoadError> {
        if targets.len() != self.imports.len() {
            return Err(LoadError::Invalid("PE import resolution count"));
        }
        let mut memory = self.image.map()?;
        for (import, target) in self.imports.iter().zip(targets) {
            let page = GuestAddress(import.slot.0 & !(PAGE - 1));
            memory.protect(page, PAGE, Protection::ReadWrite)?;
            memory.write(import.slot, &target.0.to_le_bytes())?;
            memory.protect(page, PAGE, import.protection)?;
        }
        memory.map(
            GuestAddress(STACK_TOP - STACK_SIZE - PAGE),
            PAGE,
            Protection::Guard,
        )?;
        memory.map(
            GuestAddress(STACK_TOP - STACK_SIZE),
            STACK_SIZE,
            Protection::ReadWrite,
        )?;
        let sp = STACK_TOP - 40;
        memory.write(GuestAddress(sp), &return_address.0.to_le_bytes())?;
        let mut state = X86State {
            rip: self.image.metadata.entry,
            ..Default::default()
        };
        state.gpr[4] = sp;
        Ok((memory, state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::GuestMemoryRead;

    const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/windows/hello.exe");

    #[test]
    fn imports_are_resolved_in_guest_memory() {
        let pe = parse(FIXTURE).unwrap();
        assert_eq!(
            pe.imports
                .iter()
                .map(|import| import.symbol)
                .collect::<Vec<_>>(),
            ["ExitProcess", "GetStdHandle", "WriteFile"]
        );
        let targets = [
            GuestAddress(0x1000),
            GuestAddress(0x1010),
            GuestAddress(0x1020),
        ];
        let (memory, state) = pe.start(&targets, GuestAddress(0x1030)).unwrap();
        assert_eq!(state.gpr[4] % 16, 8);
        for (import, target) in pe.imports.iter().zip(targets) {
            let mut address = [0; 8];
            memory.read(import.slot, &mut address).unwrap();
            assert_eq!(u64::from_le_bytes(address), target.0);
        }
    }

    #[test]
    fn rejects_truncated_and_out_of_range_headers() {
        for size in 0..FIXTURE.len() {
            assert!(
                parse(&FIXTURE[..size]).is_err(),
                "accepted prefix of {size} bytes"
            );
        }
        let mut invalid = FIXTURE.to_vec();
        invalid[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&invalid).is_err());
    }
}
