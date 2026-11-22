//! Linux host services.

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[allow(unsafe_code)]
pub mod jit;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[allow(unsafe_code)]
pub fn page_size() -> Option<usize> {
    // SAFETY: sysconf with this selector reads a host constant and takes no pointer.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(size).ok().filter(|&size| size != 0)
}
