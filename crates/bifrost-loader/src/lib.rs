//! Common checked binary reads and an executable image map.
use bifrost_core::{GuestAddress, GuestIsa};
use bifrost_memory::{AddressSpace, GuestMemory, MemoryError, Protection};
use std::fmt;

pub const PAGE: u64 = 4096;
pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Elf,
    Pe,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub format: Format,
    pub isa: GuestIsa,
    pub entry: GuestAddress,
}

#[derive(Clone, Copy, Debug)]
pub struct Segment<'a> {
    pub address: GuestAddress,
    pub size: u64,
    pub data: &'a [u8],
    pub protection: Protection,
}
#[derive(Debug)]
pub struct Image<'a> {
    pub metadata: Metadata,
    pub segments: Vec<Segment<'a>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadError {
    Truncated,
    Invalid(&'static str),
    Unsupported(&'static str),
    Memory(MemoryError),
}
impl From<MemoryError> for LoadError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}
impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated executable"),
            Self::Invalid(detail) => write!(f, "invalid executable: {detail}"),
            Self::Unsupported(detail) => write!(f, "unsupported executable: {detail}"),
            Self::Memory(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for LoadError {}

pub fn slice(bytes: &[u8], offset: u64, len: u64) -> Result<&[u8], LoadError> {
    let end = offset.checked_add(len).ok_or(LoadError::Truncated)?;
    let start = usize::try_from(offset).map_err(|_| LoadError::Truncated)?;
    let end = usize::try_from(end).map_err(|_| LoadError::Truncated)?;
    bytes.get(start..end).ok_or(LoadError::Truncated)
}
pub fn u16_at(bytes: &[u8], offset: u64) -> Result<u16, LoadError> {
    Ok(u16::from_le_bytes(
        slice(bytes, offset, 2)?.try_into().unwrap(),
    ))
}
pub fn u32_at(bytes: &[u8], offset: u64) -> Result<u32, LoadError> {
    Ok(u32::from_le_bytes(
        slice(bytes, offset, 4)?.try_into().unwrap(),
    ))
}
pub fn u64_at(bytes: &[u8], offset: u64) -> Result<u64, LoadError> {
    Ok(u64::from_le_bytes(
        slice(bytes, offset, 8)?.try_into().unwrap(),
    ))
}

impl Image<'_> {
    /// Maps into a fresh space. A rejected image cannot leave caller-owned mappings modified.
    pub fn map(&self) -> Result<AddressSpace, LoadError> {
        let mut memory = AddressSpace::new(PAGE as usize, (MAX_IMAGE_BYTES / PAGE) as usize)?;
        let mut ranges = Vec::new();
        let mut total = 0_u64;
        let mut valid_entry = false;
        for segment in &self.segments {
            let addr = segment.address.0;
            let end = addr
                .checked_add(segment.size)
                .ok_or(LoadError::Invalid("segment range"))?;
            let file = segment.data.len() as u64;
            if addr < PAGE
                || end > 0x0000_8000_0000_0000
                || segment.size == 0
                || file > segment.size
            {
                return Err(LoadError::Invalid("segment bounds"));
            }
            let start_page = addr & !(PAGE - 1);
            let end_page = end.next_multiple_of(PAGE);
            let length = end_page - start_page;
            total = total
                .checked_add(length)
                .filter(|&n| n <= MAX_IMAGE_BYTES - 512 * 1024)
                .ok_or(LoadError::Unsupported("total mapped image size"))?;
            ranges.push((start_page, end_page, segment));
            if matches!(
                segment.protection,
                Protection::Execute | Protection::ReadExecute
            ) && (addr..addr + file).contains(&self.metadata.entry.0)
            {
                valid_entry = true;
            }
        }
        if !valid_entry {
            return Err(LoadError::Invalid(
                "entry outside file-backed executable segment",
            ));
        }
        ranges.sort_unstable_by_key(|range| range.0);
        if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(LoadError::Unsupported("overlapping load pages"));
        }
        for (start, end, segment) in ranges {
            memory.map(GuestAddress(start), end - start, Protection::ReadWrite)?;
            memory.write(segment.address, segment.data)?;
            memory.protect(GuestAddress(start), end - start, segment.protection)?;
        }
        Ok(memory)
    }
}
