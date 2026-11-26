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
    // Empty until a write materializes this zero-filled page.
    bytes: Vec<u8>,
    protection: Protection,
    generation: u64,
    loan: Option<PageLoan>,
    permission_locked: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageLoan {
    Source,
    Destination { source: GuestAddress },
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
    loaned_pages: usize,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    pub start: GuestAddress,
    /// Exclusive end. `None` denotes the end of the 64-bit address space.
    pub end: Option<GuestAddress>,
    /// `None` denotes an unmapped region, distinct from a mapped guard or no-access page.
    pub protection: Option<Protection>,
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
            loaned_pages: 0,
            generation: 0,
        })
    }

    pub const fn page_size(&self) -> usize {
        self.page_size
    }

    /// The number of committed guest pages, including zero-filled and guard pages.
    /// A lent page counts once.
    pub fn mapped_pages(&self) -> usize {
        self.pages.len() - self.loaned_pages
    }

    pub const fn page_capacity(&self) -> usize {
        self.max_pages
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

    pub fn page_loan_at(&self, address: GuestAddress) -> Option<PageLoan> {
        self.pages.get(&self.page_base(address.0))?.loan
    }

    pub fn permission_locked_at(&self, address: GuestAddress) -> bool {
        self.pages
            .get(&self.page_base(address.0))
            .is_some_and(|page| page.permission_locked)
    }

    /// Returns the maximal contiguous interval with the page's current protection.
    pub fn region_at(&self, address: GuestAddress) -> MemoryRegion {
        use std::ops::Bound::{Excluded, Unbounded};

        let base = self.page_base(address.0);
        let width = self.page_size as u64;
        let Some(page) = self.pages.get(&base) else {
            let start = self
                .pages
                .range(..base)
                .next_back()
                .map_or(0, |(&key, _)| key + width);
            let end = self
                .pages
                .range(base..)
                .next()
                .map(|(&key, _)| GuestAddress(key));
            return MemoryRegion {
                start: GuestAddress(start),
                end,
                protection: None,
            };
        };
        let same_mapping = |other: &Page| {
            other.protection == page.protection
                && other.permission_locked == page.permission_locked
                && other.loan.as_ref().map(std::mem::discriminant)
                    == page.loan.as_ref().map(std::mem::discriminant)
        };
        let mut start = base;
        for (&key, previous) in self.pages.range(..base).rev() {
            if key.checked_add(width) != Some(start) || !same_mapping(previous) {
                break;
            }
            start = key;
        }
        let mut end = base.checked_add(width);
        for (&key, next) in self.pages.range((Excluded(base), Unbounded)) {
            if end != Some(key) || !same_mapping(next) {
                break;
            }
            end = key.checked_add(width);
        }
        MemoryRegion {
            start: GuestAddress(start),
            end: end.map(GuestAddress),
            protection: Some(page.protection),
        }
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
        if count > self.max_pages - self.mapped_pages() {
            return Err(MemoryError::PageLimit);
        }
        for base in bases.clone() {
            if self.pages.contains_key(&base) {
                return Err(MemoryError::AlreadyMapped(GuestAddress(base)));
            }
        }
        let generation = self.next_generation()?;
        for base in bases {
            self.pages.insert(
                base,
                Page {
                    bytes: Vec::new(),
                    protection: protection(base),
                    generation,
                    loan: None,
                    permission_locked: false,
                },
            );
        }
        self.generation = generation;
        Ok(())
    }

    /// Move the backing of read-write pages to a vacant range, reserving the
    /// source addresses with no access until the pages are returned.
    pub fn lend_pages(
        &mut self,
        source: GuestAddress,
        destination: GuestAddress,
        len: u64,
    ) -> Result<(), MemoryError> {
        let pairs = self
            .region(source, len)?
            .zip(self.region(destination, len)?);
        for (from, to) in pairs.clone() {
            let page = self
                .pages
                .get(&from)
                .ok_or(MemoryError::Unmapped(GuestAddress(from)))?;
            if page.protection != Protection::ReadWrite
                || page.loan.is_some()
                || page.permission_locked
            {
                return Err(MemoryError::InvalidPageLoan(GuestAddress(from)));
            }
            if self.pages.contains_key(&to) {
                return Err(MemoryError::AlreadyMapped(GuestAddress(to)));
            }
        }
        let generation = self.next_generation()?;
        for (from, to) in pairs {
            let source = self.pages.get_mut(&from).expect("validated source page");
            let bytes = std::mem::take(&mut source.bytes);
            source.protection = Protection::None;
            source.loan = Some(PageLoan::Source);
            source.generation = generation;
            self.pages.insert(
                to,
                Page {
                    bytes,
                    protection: Protection::ReadWrite,
                    generation,
                    loan: Some(PageLoan::Destination {
                        source: GuestAddress(from),
                    }),
                    permission_locked: false,
                },
            );
            self.loaned_pages += 1;
        }
        self.generation = generation;
        Ok(())
    }

    /// Return any page-aligned subrange to its original addresses.
    pub fn return_pages(
        &mut self,
        source: GuestAddress,
        destination: GuestAddress,
        len: u64,
    ) -> Result<(), MemoryError> {
        let pairs = self
            .region(source, len)?
            .zip(self.region(destination, len)?);
        for (from, to) in pairs.clone() {
            let expected = PageLoan::Destination {
                source: GuestAddress(from),
            };
            if self.page_loan_at(GuestAddress(to)) != Some(expected) {
                return Err(MemoryError::InvalidPageLoan(GuestAddress(to)));
            }
        }
        let generation = self.next_generation()?;
        for (from, to) in pairs {
            let destination = self.pages.remove(&to).expect("validated destination page");
            let source = self.pages.get_mut(&from).expect("reserved source page");
            *source = Page {
                generation,
                loan: None,
                ..destination
            };
            self.loaned_pages -= 1;
        }
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
        for base in bases.clone() {
            if self.pages[&base].permission_locked {
                return Err(MemoryError::PermissionLocked(GuestAddress(base)));
            }
        }
        let generation = self.next_generation()?;
        for base in bases {
            let page = self.pages.get_mut(&base).expect("validated mapped page");
            page.protection = protection;
            page.generation = generation;
        }
        self.generation = generation;
        Ok(())
    }

    /// Lock or unlock permission changes without altering access to the range.
    pub fn set_permission_lock(
        &mut self,
        start: GuestAddress,
        len: u64,
        locked: bool,
    ) -> Result<(), MemoryError> {
        let bases = self.region(start, len)?;
        self.check_mapped(bases.clone())?;
        for base in bases {
            self.pages
                .get_mut(&base)
                .expect("validated mapped page")
                .permission_locked = locked;
        }
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
            let page = self
                .pages
                .get(&base)
                .ok_or(MemoryError::Unmapped(GuestAddress(base)))?;
            if page.loan.is_some() {
                return Err(MemoryError::InvalidPageLoan(GuestAddress(base)));
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
            let bytes = &self.pages[&base].bytes;
            let output = &mut output[done..done + count];
            if bytes.is_empty() {
                output.fill(0);
            } else {
                output.copy_from_slice(&bytes[offset..offset + count]);
            }
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
        let mut new_pages = Vec::new();
        for (base, _, done, count) in self.chunks(address, bytes.len())? {
            if self.pages[&base].bytes.is_empty()
                && bytes[done..done + count].iter().any(|&byte| byte != 0)
            {
                let mut page = Vec::new();
                page.try_reserve_exact(self.page_size)
                    .map_err(|_| MemoryError::Allocation)?;
                page.resize(self.page_size, 0);
                new_pages
                    .try_reserve(1)
                    .map_err(|_| MemoryError::Allocation)?;
                new_pages.push((base, page));
            }
        }
        for (base, bytes) in new_pages {
            self.pages
                .get_mut(&base)
                .expect("validated writable page")
                .bytes = bytes;
        }
        for (base, offset, done, count) in self.chunks(address, bytes.len())? {
            let page = self.pages.get_mut(&base).expect("validated writable page");
            if !page.bytes.is_empty() {
                page.bytes[offset..offset + count].copy_from_slice(&bytes[done..done + count]);
            }
            page.generation = generation;
        }
        self.generation = generation;
        Ok(())
    }
}
