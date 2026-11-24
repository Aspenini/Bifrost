use bifrost_core::{GuestAddress, GuestMemory, Protection};
use bifrost_loader::{Image, LoadError, PAGE, slice, u64_at};
use bifrost_memory::AddressSpace;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const LOAD_BIAS: u64 = 0x0040_0000_0000;
const MAX_RELOCATIONS: u64 = 4096;
const RELA_BYTES: u64 = 24;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;
const DT_RELACOUNT: u64 = 0x6fff_fff9;
const DT_FLAGS_1: u64 = 0x6fff_fffb;
const DF_1_PIE: u64 = 0x0800_0000;
const R_X86_64_RELATIVE: u64 = 8;

pub(super) struct Rebase {
    relocations: Vec<(GuestAddress, u64)>,
    relro: Option<(GuestAddress, u64)>,
}

impl Rebase {
    pub(super) fn parse(
        image: &Image<'_>,
        address: u64,
        dynamic: &[u8],
        relro: Option<(u64, u64)>,
    ) -> Result<Self, LoadError> {
        if !std::ptr::eq(file_view(image, address, dynamic.len() as u64)?, dynamic) {
            return Err(LoadError::Invalid("ELF dynamic segment file mapping"));
        }
        if dynamic.is_empty() || !dynamic.len().is_multiple_of(16) || dynamic.len() > PAGE as usize
        {
            return Err(LoadError::Invalid("ELF dynamic table size"));
        }
        let mut tags = BTreeMap::new();
        let mut terminated = false;
        for row in dynamic.chunks_exact(16) {
            let tag = u64_at(row, 0)?;
            let value = u64_at(row, 8)?;
            if tag == 0 {
                terminated = true;
                break;
            }
            match tag {
                DT_RELA | DT_RELASZ | DT_RELAENT | DT_RELACOUNT | DT_FLAGS_1 => {
                    if tags.insert(tag, value).is_some() {
                        return Err(LoadError::Invalid("duplicate ELF dynamic tag"));
                    }
                }
                // Linker-produced hash and symbol metadata. This profile never uses these tables.
                4 | 5 | 6 | 10 | 11 | 0x6fff_fef5 => {}
                21 if value == 0 => {} // DT_DEBUG
                _ => return Err(LoadError::Unsupported("ELF dynamic dependency or tag")),
            }
        }
        if !terminated {
            return Err(LoadError::Invalid("unterminated ELF dynamic table"));
        }
        if tags.get(&DT_FLAGS_1) != Some(&DF_1_PIE) {
            return Err(LoadError::Unsupported(
                "ELF ET_DYN requires the PIE flag only",
            ));
        }
        let table = match (
            tags.get(&DT_RELA),
            tags.get(&DT_RELASZ),
            tags.get(&DT_RELAENT),
        ) {
            (None, None, None) if !tags.contains_key(&DT_RELACOUNT) => &[][..],
            (Some(&address), Some(&size), Some(&RELA_BYTES))
                if size.is_multiple_of(RELA_BYTES)
                    && size / RELA_BYTES <= MAX_RELOCATIONS
                    && tags
                        .get(&DT_RELACOUNT)
                        .is_none_or(|&count| count <= size / RELA_BYTES) =>
            {
                file_view(image, biased(address)?, size)?
            }
            _ => return Err(LoadError::Invalid("ELF RELA table dimensions")),
        };
        let mut relocations = Vec::new();
        let mut destinations = BTreeSet::new();
        for row in table.chunks_exact(RELA_BYTES as usize) {
            if u64_at(row, 8)? != R_X86_64_RELATIVE {
                return Err(LoadError::Unsupported(
                    "ELF relocation requires symbol-free R_X86_64_RELATIVE",
                ));
            }
            let destination = biased(u64_at(row, 0)?)?;
            if !destination.is_multiple_of(8)
                || !writable_range(image, destination, 8)
                || !destinations.insert(destination)
            {
                return Err(LoadError::Invalid("ELF relocation destination"));
            }
            let value = LOAD_BIAS
                .checked_add_signed(u64_at(row, 16)? as i64)
                .filter(|&addr| {
                    image
                        .segments
                        .iter()
                        .any(|segment| span_contains(segment.address.0, segment.size, addr, 0))
                })
                .ok_or(LoadError::Invalid("ELF relocation addend"))?;
            relocations.push((GuestAddress(destination), value));
        }
        let relro = if let Some((address, size)) = relro {
            if size == 0
                || !address.is_multiple_of(PAGE)
                || !size.is_multiple_of(PAGE)
                || !writable_range(image, address, size)
            {
                return Err(LoadError::Unsupported(
                    "ELF RELRO requires complete writable load pages",
                ));
            }
            Some((GuestAddress(address), size))
        } else {
            None
        };
        Ok(Self { relocations, relro })
    }

    pub(super) fn apply(&self, memory: &mut AddressSpace) -> Result<(), LoadError> {
        for &(address, value) in &self.relocations {
            memory.write(address, &value.to_le_bytes())?;
        }
        if let Some((address, size)) = self.relro {
            memory.protect(address, size, Protection::Read)?;
        }
        Ok(())
    }
}

fn biased(address: u64) -> Result<u64, LoadError> {
    LOAD_BIAS
        .checked_add(address)
        .ok_or(LoadError::Invalid("ELF PIE address overflow"))
}

fn span_contains(start: u64, len: u64, address: u64, size: u64) -> bool {
    address
        .checked_sub(start)
        .and_then(|offset| offset.checked_add(size))
        .is_some_and(|end| end <= len)
}

fn writable_range(image: &Image<'_>, address: u64, size: u64) -> bool {
    image.segments.iter().any(|segment| {
        segment.protection == Protection::ReadWrite
            && span_contains(segment.address.0, segment.size, address, size)
    })
}

fn file_view<'a>(image: &Image<'a>, address: u64, size: u64) -> Result<&'a [u8], LoadError> {
    for segment in &image.segments {
        if matches!(
            segment.protection,
            Protection::Read | Protection::ReadWrite | Protection::ReadExecute
        ) && span_contains(segment.address.0, segment.data.len() as u64, address, size)
        {
            return slice(segment.data, address - segment.address.0, size);
        }
    }
    Err(LoadError::Invalid(
        "ELF table outside file-backed readable segment",
    ))
}
