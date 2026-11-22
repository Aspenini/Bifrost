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
