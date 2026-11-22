//! Windows standard stream writes for the native console provider.
use std::io;
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::WriteFile,
    System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE},
};

pub fn write_standard(stderr: bool, bytes: &[u8]) -> io::Result<usize> {
    let len =
        u32::try_from(bytes.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut written = 0;
    // SAFETY: The standard handle is borrowed for synchronous I/O. Only the host slice and
    // initialized count pointer are passed to Windows.
    unsafe {
        let handle = GetStdHandle(if stderr {
            STD_ERROR_HANDLE
        } else {
            STD_OUTPUT_HANDLE
        });
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        if handle.is_null() {
            return Err(io::Error::from(io::ErrorKind::NotConnected));
        }
        if WriteFile(
            handle,
            bytes.as_ptr(),
            len,
            &mut written,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(written as usize)
}
