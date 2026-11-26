use crate::{AccessKind, FaultClass, GuestAddress, GuestFault, GuestIsa, GuestThreadId};
use std::fmt;
use std::num::NonZeroU8;

/// Checks the span and alignment of an architectural data access before consulting page mappings.
pub fn validate_guest_access(
    isa: GuestIsa,
    address: GuestAddress,
    width: NonZeroU8,
    alignment: NonZeroU8,
) -> Result<(), (GuestAddress, FaultClass)> {
    if isa == GuestIsa::X86_64 && !address.is_canonical_48() {
        return Err((address, FaultClass::NonCanonicalAddress));
    }
    let last = address
        .checked_add(u64::from(width.get() - 1))
        .ok_or((address, FaultClass::AddressOverflow))?;
    if isa == GuestIsa::X86_64 && !last.is_canonical_48() {
        return Err((
            GuestAddress(0x0000_8000_0000_0000),
            FaultClass::NonCanonicalAddress,
        ));
    }
    if !address.0.is_multiple_of(u64::from(alignment.get())) {
        return Err((address, FaultClass::Alignment));
    }
    Ok(())
}

/// Reads and instruction fetches leave the output untouched on error.
/// Implementations must enforce the corresponding permissions across the entire span.
pub trait GuestMemoryRead {
    fn read(&self, address: GuestAddress, output: &mut [u8]) -> Result<(), MemoryError>;
    fn fetch(&self, address: GuestAddress, output: &mut [u8]) -> Result<(), MemoryError>;
}

/// Writes are all-or-nothing on error. Mutation requires exclusive access.
pub trait GuestMemory: GuestMemoryRead {
    fn write(&mut self, address: GuestAddress, bytes: &[u8]) -> Result<(), MemoryError>;

    /// Checks an entire span for writing without changing guest bytes or page generations.
    fn check_write(&self, _address: GuestAddress, _len: usize) -> Result<(), MemoryError> {
        Err(MemoryError::WriteCheckUnavailable)
    }

    /// Returns the previous value and writes the replacement only on a match.
    /// Shared backing stores must override this method to serialize the entire operation.
    fn compare_exchange_le(
        &mut self,
        address: GuestAddress,
        width: usize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        let previous = read_atomic_value(self, address, width)?;
        if previous[..width] == expected.to_le_bytes()[..width] {
            self.write(address, &replacement.to_le_bytes()[..width])?;
        }
        Ok(u64::from_le_bytes(previous))
    }

    /// Adds to a little-endian scalar with width-specific wrapping and returns the old value.
    /// Shared backing stores must override this method to serialize the entire operation.
    fn fetch_add_le(
        &mut self,
        address: GuestAddress,
        width: usize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        let previous = u64::from_le_bytes(read_atomic_value(self, address, width)?);
        self.write(
            address,
            &previous.wrapping_add(addend).to_le_bytes()[..width],
        )?;
        Ok(previous)
    }
}

fn read_atomic_value(
    memory: &(impl GuestMemory + ?Sized),
    address: GuestAddress,
    width: usize,
) -> Result<[u8; 8], MemoryError> {
    if !matches!(width, 1 | 2 | 4 | 8) {
        return Err(MemoryError::InvalidRegion {
            start: address,
            len: width as u64,
        });
    }
    memory.check_write(address, width)?;
    let mut previous = [0; 8];
    memory.read(address, &mut previous[..width])?;
    Ok(previous)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Protection {
    None,
    Guard,
    Read,
    Write,
    Execute,
    ReadWrite,
    ReadExecute,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MemoryError {
    InvalidPageSize(usize),
    InvalidRegion {
        start: GuestAddress,
        len: u64,
    },
    AddressOverflow(GuestAddress),
    Unmapped(GuestAddress),
    AlreadyMapped(GuestAddress),
    InvalidPageLoan(GuestAddress),
    PermissionLocked(GuestAddress),
    Denied {
        address: GuestAddress,
        access: AccessKind,
        protection: Protection,
    },
    PageLimit,
    Allocation,
    GenerationExhausted,
    AddressSpaceIdentityExhausted,
    WriteCheckUnavailable,
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPageSize(size) => write!(f, "invalid guest page size {size}"),
            Self::InvalidRegion { start, len } => {
                write!(f, "invalid page range at {:#x} ({} bytes)", start.0, len)
            }
            Self::AddressOverflow(start) => write!(f, "guest range overflows at {:#x}", start.0),
            Self::Unmapped(address) => write!(f, "unmapped guest address {:#x}", address.0),
            Self::AlreadyMapped(address) => {
                write!(f, "guest page at {:#x} is already mapped", address.0)
            }
            Self::InvalidPageLoan(address) => {
                write!(f, "invalid guest page loan at {:#x}", address.0)
            }
            Self::PermissionLocked(address) => {
                write!(f, "guest page permissions are locked at {:#x}", address.0)
            }
            Self::Denied {
                address,
                access,
                protection,
            } => write!(
                f,
                "{access:?} denied at guest address {:#x} ({protection:?})",
                address.0
            ),
            Self::PageLimit => f.write_str("guest page limit exceeded"),
            Self::Allocation => f.write_str("could not allocate guest memory"),
            Self::GenerationExhausted => f.write_str("guest page generations exhausted"),
            Self::AddressSpaceIdentityExhausted => {
                f.write_str("guest address-space identities exhausted")
            }
            Self::WriteCheckUnavailable => {
                f.write_str("guest memory does not support a non-mutating write check")
            }
        }
    }
}

impl std::error::Error for MemoryError {}

impl MemoryError {
    /// Converts guest access violations, preserving the precise address reported by memory.
    pub const fn guest_fault(
        self,
        pc: GuestAddress,
        thread: GuestThreadId,
        access: AccessKind,
    ) -> Option<GuestFault> {
        let (address, class) = match self {
            Self::AddressOverflow(address) => (address, FaultClass::AddressOverflow),
            Self::Unmapped(address) => (address, FaultClass::Unmapped),
            Self::Denied {
                address,
                protection: Protection::Guard,
                ..
            } => (address, FaultClass::Guard),
            Self::Denied { address, .. } => (address, FaultClass::Protection),
            _ => return None,
        };
        Some(GuestFault {
            pc,
            thread,
            class,
            memory_access: Some((address, access)),
        })
    }
}
