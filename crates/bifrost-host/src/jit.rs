//! Host-owned executable memory for generated code.

use std::{io, ptr::NonNull};

pub trait ExecutableImage {
    fn entry(&self) -> NonNull<u8>;
}

/// Publishes machine code in an owned executable mapping.
///
/// # Safety
///
/// Each successful image must expose the supplied bytes at its entry address. The mapping must
/// remain executable and immutable, with no writable aliases, until its owner is dropped.
/// Publication must perform any instruction-cache synchronization the host requires.
pub unsafe trait JitMemoryManager {
    type Image: ExecutableImage;

    fn publish(&self, code: &[u8]) -> io::Result<Self::Image>;
}
