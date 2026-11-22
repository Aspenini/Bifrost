use bifrost_host::jit::{ExecutableImage, JitMemoryManager};
use std::{io, ptr::NonNull};

pub struct LinuxJitMemory;

pub struct LinuxImage {
    base: NonNull<u8>,
    len: usize,
}

impl ExecutableImage for LinuxImage {
    fn entry(&self) -> NonNull<u8> {
        self.base
    }
}

impl Drop for LinuxImage {
    fn drop(&mut self) {
        // SAFETY: This is the sole owner of the mmap allocation and its length.
        unsafe {
            libc::munmap(self.base.as_ptr().cast(), self.len);
        }
    }
}

// SAFETY: The x86-64 adapter uses a fresh private mapping, copies into non-executable pages,
// then marks the mapping RX. The host uses coherent instruction and data caches.
unsafe impl JitMemoryManager for LinuxJitMemory {
    type Image = LinuxImage;

    fn publish(&self, code: &[u8]) -> io::Result<Self::Image> {
        if code.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty native code",
            ));
        }
        // SAFETY: An anonymous private mapping requires no file. A write-only request also
        // avoids READ_IMPLIES_EXEC changing the initial protection of this mapping.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                code.len(),
                libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let Some(ptr) = NonNull::new(base.cast::<u8>()) else {
            // SAFETY: mmap succeeded at zero, which cannot be represented by this image.
            unsafe {
                libc::munmap(base, code.len());
            }
            return Err(io::Error::other("mmap selected a null entry address"));
        };
        let image = LinuxImage {
            base: ptr,
            len: code.len(),
        };
        // SAFETY: The private allocation is writable and covers this exact copy.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), image.base.as_ptr(), image.len);
            if libc::mprotect(base, image.len, libc::PROT_READ | libc::PROT_EXEC) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_mapping_is_execute_read() {
        let image = LinuxJitMemory.publish(&[0xc3]).unwrap();
        let address = image.entry().as_ptr().addr();
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let protection = maps.lines().find_map(|line| {
            let mut fields = line.split_whitespace();
            let (start, end) = fields.next()?.split_once('-')?;
            let start = usize::from_str_radix(start, 16).ok()?;
            let end = usize::from_str_radix(end, 16).ok()?;
            (start..end)
                .contains(&address)
                .then(|| fields.next())
                .flatten()
        });
        assert_eq!(protection, Some("r-xp"));
    }
}
