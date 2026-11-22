//! Minimal Win32/NT console provider. Guest handles never represent host handles.
use bifrost_abi::{AbiDescriptor, AbiError, Location};
use bifrost_core::{AbiId, CpuState, GuestAddress, GuestMemory};
use std::io::Write;

pub const STUB_BASE: u64 = 0x7fff_ff00_0000;
pub const RETURN_PC: GuestAddress = GuestAddress(STUB_BASE + 0xff0);
const MAX_IO: u32 = 65536;
const HANDLE_BASE: u64 = 0x100;
const INVALID_HANDLE: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiPolicy {
    Auto,
    Portable,
    Native,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Service {
    GetStdHandle,
    WriteFile,
    CloseHandle,
    GetLastError,
    SetLastError,
    ExitProcess,
    NtTerminateProcess,
    NtClose,
}
const SERVICES: [Service; 8] = [
    Service::GetStdHandle,
    Service::WriteFile,
    Service::CloseHandle,
    Service::GetLastError,
    Service::SetLastError,
    Service::ExitProcess,
    Service::NtTerminateProcess,
    Service::NtClose,
];

pub fn resolve(module: &str, symbol: &str) -> Option<GuestAddress> {
    let module_kernel = module.eq_ignore_ascii_case("kernel32.dll")
        || module.eq_ignore_ascii_case("kernelbase.dll");
    let module_nt = module.eq_ignore_ascii_case("ntdll.dll");
    let service = match symbol {
        "GetStdHandle" if module_kernel => Service::GetStdHandle,
        "WriteFile" if module_kernel => Service::WriteFile,
        "CloseHandle" if module_kernel => Service::CloseHandle,
        "GetLastError" if module_kernel => Service::GetLastError,
        "SetLastError" if module_kernel => Service::SetLastError,
        "ExitProcess" if module_kernel => Service::ExitProcess,
        "NtTerminateProcess" if module_nt => Service::NtTerminateProcess,
        "NtClose" if module_nt => Service::NtClose,
        _ => return None,
    };
    let index = SERVICES.iter().position(|&item| item == service)?;
    Some(GuestAddress(STUB_BASE + index as u64 * 16))
}

pub fn native_available() -> bool {
    cfg!(all(target_os = "windows", target_arch = "x86_64"))
}

pub struct Io<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}
pub struct Windows<'a> {
    io: Io<'a>,
    native: bool,
    allowed: [bool; SERVICES.len()],
    handles_open: [bool; 3],
    last_error: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiAction {
    Step,
    Returned,
    Exit(u32),
}

#[derive(Debug)]
pub enum ApiError {
    NativeUnavailable,
    Abi(AbiError),
    Undeclared(GuestAddress),
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NativeUnavailable => {
                f.write_str("native Windows console provider is unavailable on this host")
            }
            Self::Abi(error) => error.fmt(f),
            Self::Undeclared(address) => {
                write!(f, "undeclared Windows import dispatch at {:#x}", address.0)
            }
        }
    }
}
impl std::error::Error for ApiError {}
impl From<AbiError> for ApiError {
    fn from(error: AbiError) -> Self {
        Self::Abi(error)
    }
}

impl<'a> Windows<'a> {
    pub fn new(io: Io<'a>, policy: ApiPolicy, imports: &[GuestAddress]) -> Result<Self, ApiError> {
        let native = policy == ApiPolicy::Native;
        if native && !native_available() {
            return Err(ApiError::NativeUnavailable);
        }
        // Auto selects the injected portable streams, so capture and redirection remain coherent.
        let mut allowed = [false; SERVICES.len()];
        for &address in imports {
            let index = service_index(address).ok_or(ApiError::Undeclared(address))?;
            allowed[index] = true;
        }
        Ok(Self {
            io,
            native,
            allowed,
            handles_open: [true; 3],
            last_error: 0,
        })
    }

    pub fn dispatch(
        &mut self,
        cpu: &mut CpuState,
        memory: &mut dyn GuestMemory,
    ) -> Result<ApiAction, ApiError> {
        let abi = AbiDescriptor::for_id(AbiId::WindowsX64)?;
        if cpu.pc() == RETURN_PC {
            return Ok(ApiAction::Exit(
                abi.read_result_register(cpu, Location::Integer(0))? as u32,
            ));
        }
        let Some(index) = service_index(cpu.pc()) else {
            return Ok(ApiAction::Step);
        };
        if !self.allowed[index] {
            return Err(ApiError::Undeclared(cpu.pc()));
        }
        let service = SERVICES[index];
        let resume = abi.capture_return(cpu, memory)?;
        let arg0 = abi.read_word(cpu, memory, Location::Integer(1))?;
        let result = match service {
            Service::GetStdHandle => {
                let which = match arg0 as u32 as i32 {
                    -10 => Some(0),
                    -11 => Some(1),
                    -12 => Some(2),
                    _ => None,
                };
                match which {
                    Some(which) if self.handles_open[which] => HANDLE_BASE + which as u64,
                    _ => {
                        self.last_error = 6;
                        INVALID_HANDLE
                    }
                }
            }
            Service::WriteFile => {
                let address = GuestAddress(abi.read_word(cpu, memory, Location::Integer(2))?);
                let len = abi.read_word(cpu, memory, Location::Integer(8))? as u32;
                let written = GuestAddress(abi.read_word(cpu, memory, Location::Integer(9))?);
                let overlap = abi.read_word(cpu, memory, Location::Stack(40))?;
                self.write_file(
                    memory,
                    abi,
                    WriteRequest {
                        handle: arg0,
                        address,
                        len,
                        written,
                        overlap,
                    },
                )
            }
            Service::CloseHandle => {
                if self.close(arg0) {
                    1
                } else {
                    self.failure(6)
                }
            }
            Service::GetLastError => self.last_error.into(),
            Service::SetLastError => {
                self.last_error = arg0 as u32;
                0
            }
            Service::ExitProcess => return Ok(ApiAction::Exit(arg0 as u32)),
            Service::NtTerminateProcess => {
                if arg0 == u64::MAX {
                    let code = abi.read_word(cpu, memory, Location::Integer(2))?;
                    return Ok(ApiAction::Exit(code as u32));
                }
                0xc000_0008
            }
            Service::NtClose => {
                if self.close(arg0) {
                    0
                } else {
                    0xc000_0008
                }
            }
        };
        abi.write_word(cpu, memory, Location::Integer(0), result)?;
        resume.apply(cpu);
        Ok(ApiAction::Returned)
    }

    fn failure(&mut self, error: u32) -> u64 {
        self.last_error = error;
        0
    }

    fn close(&mut self, id: u64) -> bool {
        let Some(index) = id
            .checked_sub(HANDLE_BASE)
            .and_then(|n| usize::try_from(n).ok())
        else {
            return false;
        };
        self.handles_open
            .get_mut(index)
            .is_some_and(|open| std::mem::replace(open, false))
    }

    fn write_file(
        &mut self,
        memory: &mut dyn GuestMemory,
        abi: &AbiDescriptor,
        request: WriteRequest,
    ) -> u64 {
        let WriteRequest {
            handle,
            address,
            len,
            written,
            overlap,
        } = request;
        let index = match handle.checked_sub(HANDLE_BASE) {
            Some(index @ (1 | 2)) if self.handles_open[index as usize] => index,
            _ => return self.failure(6),
        };
        if written.0 == 0 || overlap != 0 {
            return self.failure(87);
        }
        let len = len.min(MAX_IO) as usize;
        if abi.validate_span(written, 4).is_err() || memory.check_write(written, 4).is_err() {
            return self.failure(998);
        }
        let mut bytes = vec![0; len];
        if len != 0
            && (abi.validate_span(address, len).is_err()
                || memory.read(address, &mut bytes).is_err())
        {
            return self.failure(998);
        }
        let count = if self.native {
            native_write(index == 2, &bytes)
        } else if index == 1 {
            self.io.stdout.write(&bytes)
        } else {
            self.io.stderr.write(&bytes)
        };
        let count = match count {
            Ok(value) => value as u32,
            Err(_) => return self.failure(29),
        };
        if memory.write(written, &count.to_le_bytes()).is_err() {
            return self.failure(998);
        }
        1
    }
}

struct WriteRequest {
    handle: u64,
    address: GuestAddress,
    len: u32,
    written: GuestAddress,
    overlap: u64,
}

fn service_index(address: GuestAddress) -> Option<usize> {
    let delta = address.0.checked_sub(STUB_BASE)?;
    if !delta.is_multiple_of(16) {
        return None;
    }
    usize::try_from(delta / 16)
        .ok()
        .filter(|&index| index < SERVICES.len())
}

fn native_write(stderr: bool, bytes: &[u8]) -> std::io::Result<usize> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        bifrost_host_windows::console::write_standard(stderr, bytes)
    }
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        let _ = (stderr, bytes);
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}
