//! Host information for the macOS interpreter adapter.

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn page_size() -> Option<usize> {
    // SAFETY: sysconf with this selector reads a host constant and takes no pointer.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(size).ok().filter(|&size| size != 0)
}
