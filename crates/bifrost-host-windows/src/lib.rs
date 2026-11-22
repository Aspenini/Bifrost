//! Windows host services.

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[allow(unsafe_code)]
pub mod console;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[allow(unsafe_code)]
pub mod jit;
