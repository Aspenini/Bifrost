//! Windows host services.

#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[allow(unsafe_code)]
pub mod console;
#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[allow(unsafe_code)]
pub mod jit;

#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[allow(unsafe_code)]
pub fn page_size() -> Option<usize> {
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

    let mut info = SYSTEM_INFO::default();
    // SAFETY: info is writable storage for the fixed-size SYSTEM_INFO structure.
    unsafe { GetSystemInfo(&mut info) };
    (info.dwPageSize != 0).then_some(info.dwPageSize as usize)
}
