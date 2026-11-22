//! Version 1 of the C embedding interface.
#![allow(unsafe_code)]

mod raw;
pub use raw::{RawResult, bifrost_run_raw};

use bifrost_runtime::{RuntimeProfile, core::CapabilityProvider};
#[cfg(feature = "executable-images")]
use bifrost_runtime::{
    core::ExecMode,
    process::{self, ApiPolicy, Io, RunError, RunOptions, Vfs, WindowsIo},
};
#[cfg(feature = "executable-images")]
use std::io;
use std::{panic::catch_unwind, ptr, slice};

#[cfg(feature = "executable-images")]
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_STEPS: u64 = 1_000_000;

/// Opaque session owned by the C caller.
pub struct Session(RuntimeProfile);

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok = 0,
    InvalidArgument = 1,
    LoadFailure = 2,
    GuestFailure = 3,
    BudgetExhausted = 4,
    OutputTruncated = 5,
    InternalError = 6,
    Unavailable = 7,
}

#[repr(C)]
#[derive(Default)]
pub struct RunResult {
    pub exit_code: u32,
    pub output_written: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn bifrost_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn bifrost_create() -> *mut Session {
    catch_unwind(|| Box::into_raw(Box::new(Session(RuntimeProfile::default()))))
        .unwrap_or(ptr::null_mut())
}

/// # Safety
/// A nonnull session must be a live pointer from this library and must be exclusively owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bifrost_destroy(session: *mut Session) {
    if !session.is_null() {
        let _ = catch_unwind(|| {
            // SAFETY: The caller transfers the original allocation back exactly once.
            drop(unsafe { Box::from_raw(session) });
        });
    }
}

/// # Safety
/// Session must remain alive throughout the call. The ID must be readable for its stated length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bifrost_capability(
    session: *const Session,
    id: *const u8,
    len: usize,
) -> u32 {
    if session.is_null() || id.is_null() || !(1..=128).contains(&len) {
        return 0;
    }
    catch_unwind(|| {
        // SAFETY: Both borrows are valid for the duration of this call.
        let (session, id) = unsafe { (&*session, slice::from_raw_parts(id, len)) };
        if id == b"c-abi" || id == b"c-abi-raw" {
            return 1 | 2 | 4 | 8;
        }
        if !cfg!(feature = "executable-images") && matches!(id, b"linux-elf" | b"windows-pe") {
            return 1;
        }
        let Some(item) = std::str::from_utf8(id)
            .ok()
            .and_then(|id| session.0.capabilities().get(id))
        else {
            return 0;
        };
        1 | (u32::from(item.compiled()) << 1)
            | (u32::from(item.implemented_on_any_host()) << 2)
            | (u32::from(item.host_available()) << 3)
            | (u32::from(item.validated()) << 4)
    })
    .unwrap_or(0)
}

/// Runs an ELF or PE image using the interpreter. Guest stdout and stderr are combined.
///
/// # Safety
/// The session is live. The image is readable for its stated length. The output and result
/// are writable, correctly aligned, and disjoint from each other and all readable inputs.
#[unsafe(no_mangle)]
#[cfg(feature = "executable-images")]
pub unsafe extern "C" fn bifrost_run(
    session: *const Session,
    image: *const u8,
    image_len: usize,
    instruction_limit: u64,
    output: *mut u8,
    output_len: usize,
    result: *mut RunResult,
) -> Status {
    if session.is_null()
        || image.is_null()
        || image_len == 0
        || image_len > MAX_BYTES
        || instruction_limit == 0
        || instruction_limit > MAX_STEPS
        || output_len > MAX_BYTES
        || (output_len != 0 && output.is_null())
        || result.is_null()
    {
        return Status::InvalidArgument;
    }
    catch_unwind(|| {
        // SAFETY: The caller provides a readable range and a disjoint initialized-on-write result.
        let image = unsafe { slice::from_raw_parts(image, image_len) };
        let result = unsafe {
            result.write(RunResult::default());
            &mut *result
        };
        let output = if output_len == 0 {
            &mut []
        } else {
            // SAFETY: The caller provides a disjoint writable range; bytes may be uninitialized.
            unsafe {
                slice::from_raw_parts_mut(output.cast::<std::mem::MaybeUninit<u8>>(), output_len)
            }
        };
        let capture = std::cell::RefCell::new(Capture {
            output,
            written: 0,
            truncated: false,
        });
        let (mut stdout, mut stderr) = (Stream(&capture), Stream(&capture));
        let options = RunOptions {
            execution: ExecMode::Interpreter,
            instruction_limit,
        };
        let outcome = if image.starts_with(b"MZ") {
            process::run_windows(
                image,
                WindowsIo {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                ApiPolicy::Portable,
                options,
            )
        } else {
            process::run_linux(
                image,
                &[b"guest"],
                &[],
                Io {
                    input: &[],
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                &Vfs::default(),
                options,
            )
        };
        let capture = capture.into_inner();
        result.output_written = capture.written;
        match outcome {
            Ok(exit_code) => {
                result.exit_code = exit_code;
                if capture.truncated {
                    Status::OutputTruncated
                } else {
                    Status::Ok
                }
            }
            Err(RunError::Load(_)) => Status::LoadFailure,
            Err(RunError::Budget) => Status::BudgetExhausted,
            Err(_) => Status::GuestFailure,
        }
    })
    .unwrap_or(Status::InternalError)
}

/// Reports that executable image support was disabled in this build.
///
/// # Safety
/// The session and buffer arguments follow the executable-image ABI contract.
#[unsafe(no_mangle)]
#[cfg(not(feature = "executable-images"))]
pub unsafe extern "C" fn bifrost_run(
    _: *const Session,
    _: *const u8,
    _: usize,
    _: u64,
    _: *mut u8,
    _: usize,
    _: *mut RunResult,
) -> Status {
    Status::Unavailable
}

#[cfg(feature = "executable-images")]
struct Capture<'a> {
    output: &'a mut [std::mem::MaybeUninit<u8>],
    written: usize,
    truncated: bool,
}

#[cfg(feature = "executable-images")]
struct Stream<'a, 'b>(&'a std::cell::RefCell<Capture<'b>>);

#[cfg(feature = "executable-images")]
impl io::Write for Stream<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut capture = self.0.borrow_mut();
        let start = capture.written;
        let count = bytes.len().min(capture.output.len() - start);
        for (slot, &byte) in capture.output[start..start + count].iter_mut().zip(bytes) {
            slot.write(byte);
        }
        capture.written += count;
        capture.truncated |= count < bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
