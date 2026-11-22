use bifrost_host::jit::{ExecutableImage, JitMemoryManager};
use std::{io, ptr::NonNull};
use windows_sys::Win32::System::{
    Diagnostics::Debug::FlushInstructionCache,
    Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc,
        VirtualFree, VirtualProtect,
    },
    Threading::GetCurrentProcess,
};

pub struct WindowsJitMemory;

pub struct WindowsImage(NonNull<u8>);

impl ExecutableImage for WindowsImage {
    fn entry(&self) -> NonNull<u8> {
        self.0
    }
}

impl Drop for WindowsImage {
    fn drop(&mut self) {
        // SAFETY: This owns the original VirtualAlloc base, which is released once.
        unsafe {
            VirtualFree(self.0.as_ptr().cast(), 0, MEM_RELEASE);
        }
    }
}

// SAFETY: Publication copies into a fresh RW allocation, converts it to RX and flushes
// the instruction cache before exposing it. No mutable mapping or aliased owner escapes.
unsafe impl JitMemoryManager for WindowsJitMemory {
    type Image = WindowsImage;

    fn publish(&self, code: &[u8]) -> io::Result<Self::Image> {
        if code.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty native code",
            ));
        }
        // SAFETY: A null hint requests a new allocation, with at least code.len() writable bytes.
        let base = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                code.len(),
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        let base = NonNull::new(base.cast::<u8>()).ok_or_else(io::Error::last_os_error)?;
        let image = WindowsImage(base);
        let mut previous = 0;
        // SAFETY: The live allocation has no aliases and was allocated for this exact span.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), base.as_ptr(), code.len());
            if VirtualProtect(
                base.as_ptr().cast(),
                code.len(),
                PAGE_EXECUTE_READ,
                &mut previous,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            if FlushInstructionCache(GetCurrentProcess(), base.as_ptr().cast(), code.len()) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};

    #[test]
    fn published_mapping_is_execute_read() {
        let image = WindowsJitMemory.publish(&[0xc3]).unwrap();
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: VirtualQuery writes at most the size of the provided information buffer.
        let size =
            unsafe { VirtualQuery(image.entry().as_ptr().cast(), &mut info, size_of_val(&info)) };
        assert_ne!(size, 0);
        assert_eq!(info.Protect, PAGE_EXECUTE_READ);
    }
}
