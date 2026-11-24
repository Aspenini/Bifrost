//! Guarded guest byte access and a sparse, owned address space.

use bifrost_core::{AccessKind, GuestAddress};
pub use bifrost_core::{GuestMemory, GuestMemoryRead, MemoryError, Protection};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ADDRESS_SPACE: AtomicU64 = AtomicU64::new(1);

/// Identifies a guest address space for the lifetime of the host process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AddressSpaceId(u64);

const fn permits(protection: Protection, access: AccessKind) -> bool {
    matches!(
        (protection, access),
        (
            Protection::Read | Protection::ReadWrite | Protection::ReadExecute,
            AccessKind::Read
        ) | (Protection::Write | Protection::ReadWrite, AccessKind::Write)
            | (
                Protection::Execute | Protection::ReadExecute,
                AccessKind::Execute
            )
    )
}

struct Page {
    bytes: Vec<u8>,
    protection: Protection,
    generation: u64,
}

/// Software-enforced guest pages. Memory is zero-filled on mapping and owned by this instance.
/// Each committed map, write or protection change receives a new generation.
///
/// ```
/// use bifrost_core::GuestAddress;
/// use bifrost_memory::{AddressSpace, GuestMemory, GuestMemoryRead, Protection};
///
/// let mut memory = AddressSpace::new(4096, 2)?;
/// let address = GuestAddress(0x1000);
/// memory.map(address, 4096, Protection::ReadWrite)?;
/// memory.write(address, &[7, 11])?;
/// memory.protect(address, 4096, Protection::Read)?;
/// let mut data = [0; 2];
/// memory.read(address, &mut data)?;
/// assert_eq!(data, [7, 11]);
/// # Ok::<(), bifrost_memory::MemoryError>(())
/// ```
pub struct AddressSpace {
    id: AddressSpaceId,
    page_size: usize,
    max_pages: usize,
    pages: BTreeMap<u64, Page>,
    generation: u64,
}

impl AddressSpace {
    pub fn new(page_size: usize, max_pages: usize) -> Result<Self, MemoryError> {
        if !page_size.is_power_of_two() {
            return Err(MemoryError::InvalidPageSize(page_size));
        }
        let id = NEXT_ADDRESS_SPACE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map(AddressSpaceId)
            .map_err(|_| MemoryError::AddressSpaceIdentityExhausted)?;
        Ok(Self {
            id,
            page_size,
            max_pages,
            pages: BTreeMap::new(),
            generation: 0,
        })
    }

    pub const fn page_size(&self) -> usize {
        self.page_size
    }

    pub const fn id(&self) -> AddressSpaceId {
        self.id
    }

    /// Returns the current page generation. Unmapping and remapping changes the generation.
    pub fn generation_at(&self, address: GuestAddress) -> Option<u64> {
        self.pages
            .get(&self.page_base(address.0))
            .map(|page| page.generation)
    }

    pub fn map(
        &mut self,
        start: GuestAddress,
        len: u64,
        protection: Protection,
    ) -> Result<(), MemoryError> {
        self.map_pages(start, len, |_| protection)
    }

    /// Maps a lower guard page and a read-write stack in one transaction.
    pub fn map_guarded_stack(
        &mut self,
        guard: GuestAddress,
        stack_bytes: u64,
    ) -> Result<GuestAddress, MemoryError> {
        let page = self.page_size as u64;
        if stack_bytes == 0 {
            return Err(MemoryError::InvalidRegion {
                start: guard,
                len: stack_bytes,
            });
        }
        let len = stack_bytes
            .checked_add(page)
            .ok_or(MemoryError::AddressOverflow(guard))?;
        let top = guard
            .checked_add(len)
            .ok_or(MemoryError::AddressOverflow(guard))?;
        self.map_pages(guard, len, |base| {
            if base == guard.0 {
                Protection::Guard
            } else {
                Protection::ReadWrite
            }
        })?;
        Ok(top)
    }

    fn map_pages(
        &mut self,
        start: GuestAddress,
        len: u64,
        protection: impl Fn(u64) -> Protection,
    ) -> Result<(), MemoryError> {
        let bases = self.region(start, len)?;
        let count =
            usize::try_from(len / self.page_size as u64).map_err(|_| MemoryError::PageLimit)?;
        if count > self.max_pages - self.pages.len() {
            return Err(MemoryError::PageLimit);
        }
        for base in bases.clone() {
            if self.pages.contains_key(&base) {
                return Err(MemoryError::AlreadyMapped(GuestAddress(base)));
            }
        }
        let generation = self.next_generation()?;
        let mut staged = Vec::new();
        staged
            .try_reserve_exact(count)
            .map_err(|_| MemoryError::Allocation)?;
        for base in bases {
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(self.page_size)
                .map_err(|_| MemoryError::Allocation)?;
            bytes.resize(self.page_size, 0);
            staged.push((
                base,
                Page {
                    bytes,
                    protection: protection(base),
                    generation,
                },
            ));
        }
        self.pages.extend(staged);
        self.generation = generation;
        Ok(())
    }

    pub fn protect(
        &mut self,
        start: GuestAddress,
        len: u64,
        protection: Protection,
    ) -> Result<(), MemoryError> {
        let bases = self.region(start, len)?;
        self.check_mapped(bases.clone())?;
        let generation = self.next_generation()?;
        for base in bases {
            let page = self.pages.get_mut(&base).expect("validated mapped page");
            page.protection = protection;
            page.generation = generation;
        }
        self.generation = generation;
        Ok(())
    }

    pub fn unmap(&mut self, start: GuestAddress, len: u64) -> Result<(), MemoryError> {
        let bases = self.region(start, len)?;
        self.check_mapped(bases.clone())?;
        // Future mappings receive a generation newer than the removed pages.
        for base in bases {
            self.pages.remove(&base);
        }
        Ok(())
    }

    fn page_base(&self, address: u64) -> u64 {
        address & !(self.page_size as u64 - 1)
    }

    fn region(
        &self,
        start: GuestAddress,
        len: u64,
    ) -> Result<impl Iterator<Item = u64> + Clone + use<>, MemoryError> {
        let size = self.page_size as u64;
        if len == 0 || !start.0.is_multiple_of(size) || !len.is_multiple_of(size) {
            return Err(MemoryError::InvalidRegion { start, len });
        }
        let last = start
            .checked_add(len - 1)
            .ok_or(MemoryError::AddressOverflow(start))?;
        Ok((start.0..=self.page_base(last.0)).step_by(self.page_size))
    }

    fn check_mapped(&self, bases: impl Iterator<Item = u64>) -> Result<(), MemoryError> {
        for base in bases {
            if !self.pages.contains_key(&base) {
                return Err(MemoryError::Unmapped(GuestAddress(base)));
            }
        }
        Ok(())
    }

    fn next_generation(&self) -> Result<u64, MemoryError> {
        self.generation
            .checked_add(1)
            .ok_or(MemoryError::GenerationExhausted)
    }

    fn validate(
        &self,
        address: GuestAddress,
        len: usize,
        access: AccessKind,
    ) -> Result<(), MemoryError> {
        for (base, offset, _, _) in self.chunks(address, len)? {
            let address = GuestAddress(base + offset as u64);
            let page = self
                .pages
                .get(&base)
                .ok_or(MemoryError::Unmapped(address))?;
            if !permits(page.protection, access) {
                return Err(MemoryError::Denied {
                    address,
                    access,
                    protection: page.protection,
                });
            }
        }
        Ok(())
    }

    /// Yields page base, offset in page, offset in the input/output slice and chunk length.
    fn chunks(
        &self,
        address: GuestAddress,
        len: usize,
    ) -> Result<impl Iterator<Item = (u64, usize, usize, usize)> + use<>, MemoryError> {
        if len != 0 {
            address
                .checked_add((len - 1) as u64)
                .ok_or(MemoryError::AddressOverflow(address))?;
        }
        let page_size = self.page_size;
        let mut done = 0;
        Ok(std::iter::from_fn(move || {
            if done == len {
                return None;
            }
            let position = address.0 + done as u64;
            let offset = (position % page_size as u64) as usize;
            let count = (len - done).min(page_size - offset);
            let chunk = (position - offset as u64, offset, done, count);
            done += count;
            Some(chunk)
        }))
    }

    fn copy_out(
        &self,
        address: GuestAddress,
        output: &mut [u8],
        access: AccessKind,
    ) -> Result<(), MemoryError> {
        self.validate(address, output.len(), access)?;
        for (base, offset, done, count) in self.chunks(address, output.len())? {
            output[done..done + count]
                .copy_from_slice(&self.pages[&base].bytes[offset..offset + count]);
        }
        Ok(())
    }
}

impl GuestMemoryRead for AddressSpace {
    fn read(&self, address: GuestAddress, output: &mut [u8]) -> Result<(), MemoryError> {
        self.copy_out(address, output, AccessKind::Read)
    }

    fn fetch(&self, address: GuestAddress, output: &mut [u8]) -> Result<(), MemoryError> {
        self.copy_out(address, output, AccessKind::Execute)
    }
}

impl GuestMemory for AddressSpace {
    fn check_write(&self, address: GuestAddress, len: usize) -> Result<(), MemoryError> {
        self.validate(address, len, AccessKind::Write)
    }

    fn write(&mut self, address: GuestAddress, bytes: &[u8]) -> Result<(), MemoryError> {
        self.validate(address, bytes.len(), AccessKind::Write)?;
        if bytes.is_empty() {
            return Ok(());
        }
        let generation = self.next_generation()?;
        for (base, offset, done, count) in self.chunks(address, bytes.len())? {
            let page = self.pages.get_mut(&base).expect("validated writable page");
            page.bytes[offset..offset + count].copy_from_slice(&bytes[done..done + count]);
            page.generation = generation;
        }
        self.generation = generation;
        Ok(())
    }
}
