//! Architecture and platform identities, guest state, and structured capability reporting.

mod capability;
mod cpu;
mod platform_api;

pub use capability::*;
pub use cpu::*;
pub use platform_api::*;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum GuestIsa {
    X86_64,
    AArch64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum GuestOs {
    None,
    Linux,
    Windows,
    MacOs,
}

/// ABI identity. Calling and layout rules belong to a descriptor for this identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum AbiId {
    Bare,
    LinuxSysvAmd64,
    LinuxAapcs64,
    WindowsX64,
    WindowsArm64,
}

impl AbiId {
    pub const fn matches(self, isa: GuestIsa, os: GuestOs) -> bool {
        matches!(
            (self, isa, os),
            (Self::Bare, _, GuestOs::None)
                | (Self::LinuxSysvAmd64, GuestIsa::X86_64, GuestOs::Linux)
                | (Self::LinuxAapcs64, GuestIsa::AArch64, GuestOs::Linux)
                | (Self::WindowsX64, GuestIsa::X86_64, GuestOs::Windows)
                | (Self::WindowsArm64, GuestIsa::AArch64, GuestOs::Windows)
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum HostIsa {
    X86_64,
    AArch64,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum HostOs {
    Windows,
    Linux,
    MacOs,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ExecutableMemoryPolicy {
    Unknown,
    Denied,
    WriteThenExecute,
    DualMapping,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostTarget {
    pub isa: HostIsa,
    pub os: HostOs,
    /// Size in bytes when discovered by a host adapter.
    pub page_size: Option<usize>,
    pub executable_memory: ExecutableMemoryPolicy,
}

impl HostTarget {
    /// Detects the compiled host identity without probing host services.
    pub const fn current() -> Self {
        let isa = if cfg!(target_arch = "x86_64") {
            HostIsa::X86_64
        } else if cfg!(target_arch = "aarch64") {
            HostIsa::AArch64
        } else {
            HostIsa::Other
        };
        let os = if cfg!(target_os = "windows") {
            HostOs::Windows
        } else if cfg!(target_os = "linux") {
            HostOs::Linux
        } else if cfg!(target_os = "macos") {
            HostOs::MacOs
        } else {
            HostOs::Other
        };
        Self {
            isa,
            os,
            page_size: None,
            executable_memory: ExecutableMemoryPolicy::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ExecMode {
    Auto,
    Native,
    Jit,
    Interpreter,
}

/// An address in guest space. It has no host pointer conversion.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GuestAddress(pub u64);

impl GuestAddress {
    pub const fn checked_add(self, bytes: u64) -> Option<Self> {
        match self.0.checked_add(bytes) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AccessKind {
    Read,
    Write,
    Execute,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GuestThreadId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FaultClass {
    AddressOverflow,
    Unmapped,
    Protection,
    Guard,
    Alignment,
    IllegalInstruction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestFault {
    pub pc: GuestAddress,
    pub thread: GuestThreadId,
    pub class: FaultClass,
    pub memory_access: Option<(GuestAddress, AccessKind)>,
}

/// A sink borrowed by the caller. Implementations may keep local event state.
pub trait TraceSink {
    fn record(&mut self, event: TraceEvent);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TraceEvent {
    Step {
        thread: GuestThreadId,
        pc: GuestAddress,
    },
    Fault(GuestFault),
}
