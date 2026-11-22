use std::fmt;

pub const MAX_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferMode {
    In,
    Out,
    InOut,
}

impl BufferMode {
    pub(crate) const fn read(self) -> bool {
        matches!(self, Self::In | Self::InOut)
    }
    pub(crate) const fn write(self) -> bool {
        matches!(self, Self::Out | Self::InOut)
    }
}

#[derive(Clone, Copy)]
pub struct NativeBuffer {
    function: unsafe extern "C" fn(*mut u8, usize) -> u64,
    pub(crate) mode: BufferMode,
    pub(crate) limit: usize,
}

impl fmt::Debug for NativeBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeBuffer")
            .field("mode", &self.mode)
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl NativeBuffer {
    /// Binds a C function to a copied, bounded guest buffer.
    ///
    /// # Safety
    ///
    /// The function may access only the supplied length and may use the pointer only for the
    /// duration of the call. It must accept a non-null dangling pointer for a zero length,
    /// avoid unwinding, and keep the bytes initialized. A write to an In buffer is discarded.
    pub unsafe fn new(
        function: unsafe extern "C" fn(*mut u8, usize) -> u64,
        mode: BufferMode,
        limit: usize,
    ) -> Self {
        Self {
            function,
            mode,
            limit: limit.min(MAX_BUFFER_BYTES),
        }
    }

    pub(crate) fn call(self, bytes: &mut [u8]) -> u64 {
        // SAFETY: This is the host-owned staging slice for the duration of this call.
        unsafe { (self.function)(bytes.as_mut_ptr(), bytes.len()) }
    }
}
