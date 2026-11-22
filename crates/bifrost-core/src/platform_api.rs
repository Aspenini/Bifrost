use crate::{AccessKind, FaultClass, GuestAddress, GuestFault, GuestThreadId};
use std::fmt;

/// Reads and instruction fetches leave the output untouched on error.
/// Implementations must enforce the corresponding permissions across the entire span.
pub trait GuestMemoryRead {
    fn read(&self, address: GuestAddress, output: &mut [u8]) -> Result<(), MemoryError>;
    fn fetch(&self, address: GuestAddress, output: &mut [u8]) -> Result<(), MemoryError>;
}

/// Writes are all-or-nothing on error. Mutation requires exclusive access.
pub trait GuestMemory: GuestMemoryRead {
    fn write(&mut self, address: GuestAddress, bytes: &[u8]) -> Result<(), MemoryError>;
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
    Denied {
        address: GuestAddress,
        access: AccessKind,
        protection: Protection,
    },
    PageLimit,
    Allocation,
    GenerationExhausted,
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
