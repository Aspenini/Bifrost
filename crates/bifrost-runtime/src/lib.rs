//! High-level capability discovery for the components selected by Cargo features.

use bifrost_core::{
    Capability, CapabilityProvider, CapabilityRegistry, ExecMode, ExecutableMemoryPolicy, GuestIsa,
    GuestOs, GuestTarget, HostTarget, SupportLevel,
};
use std::fmt;

pub use bifrost_core as core;
pub use bifrost_ir as ir;
#[cfg(feature = "macho-inspect")]
pub use bifrost_loader_macho as macho;
pub use bifrost_memory as memory;
#[cfg(any(feature = "linux-elf", feature = "windows-pe"))]
pub mod process;
#[cfg(feature = "abi-bridge")]
pub use {bifrost_abi as abi, bifrost_bridge as bridge};

fn bridge_host_available() -> bool {
    #[cfg(feature = "abi-bridge")]
    {
        bifrost_abi::AbiDescriptor::native().is_some()
    }
    #[cfg(not(feature = "abi-bridge"))]
    {
        false
    }
}

#[cfg(all(
    any(feature = "guest-x86_64", feature = "guest-aarch64"),
    any(feature = "interp", feature = "jit-cranelift")
))]
pub mod raw;

#[cfg(all(
    feature = "guest-x86_64",
    any(feature = "interp", feature = "jit-cranelift")
))]
pub mod x86;

fn guest_compiled(isa: GuestIsa) -> bool {
    (cfg!(feature = "guest-x86_64") && isa == GuestIsa::X86_64)
        || (cfg!(feature = "guest-aarch64") && isa == GuestIsa::AArch64)
}

fn supports_raw_profile(target: &GuestTarget) -> bool {
    guest_compiled(target.isa())
        && target.os() == GuestOs::None
        && target.features().iter().len() == 0
}

const JIT_HOST: bool = cfg!(all(
    feature = "jit-cranelift",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "windows")
));

fn probe_jit() -> bool {
    #[cfg(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        use bifrost_host::jit::JitMemoryManager;
        #[cfg(target_os = "linux")]
        use bifrost_host_linux::jit::LinuxJitMemory as SystemMemory;
        #[cfg(target_os = "windows")]
        use bifrost_host_windows::jit::WindowsJitMemory as SystemMemory;

        bifrost_codegen_clif::Compiler::new().is_ok() && SystemMemory.publish(&[0; 4]).is_ok()
    }
    #[cfg(not(all(
        feature = "jit-cranelift",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_os = "linux", target_os = "windows")
    )))]
    {
        false
    }
}

fn probe_page_size() -> Option<usize> {
    #[cfg(all(feature = "host-macos", target_os = "macos"))]
    return bifrost_host_macos::page_size();
    #[cfg(all(
        feature = "host-linux",
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    return bifrost_host_linux::page_size();
    #[cfg(all(
        feature = "host-windows",
        target_os = "windows",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    return bifrost_host_windows::page_size();
    #[allow(unreachable_code)]
    None
}

#[derive(Clone, Debug)]
pub struct RuntimeProfile {
    host: HostTarget,
    jit_ready: bool,
    capabilities: CapabilityRegistry,
}

impl Default for RuntimeProfile {
    fn default() -> Self {
        let jit_available = probe_jit();
        let jit_ready = jit_available;
        let mut host = HostTarget::current();
        host.page_size = probe_page_size();
        if JIT_HOST {
            host.executable_memory = if jit_available {
                ExecutableMemoryPolicy::WriteThenExecute
            } else {
                ExecutableMemoryPolicy::Denied
            };
        }
        let mut capabilities = CapabilityRegistry::default();
        for (id, detail) in [
            (
                "cpu-state",
                "x86-64 and AArch64 state and profile validation",
            ),
            ("guest-memory", "sparse owned pages and checked access"),
            ("ir-v1", "typed scalar IR and structural verification"),
        ] {
            let record = Capability::implemented(id, true, SupportLevel::Partial, detail)
                .expect("starter components have an implemented support level");
            capabilities
                .register(record)
                .expect("unique built-in capability");
        }
        for (id, feature, enabled, detail) in [
            (
                "decode-x86_64",
                "enable guest-x86_64",
                cfg!(feature = "guest-x86_64"),
                "64-bit decoding with normalized operands for the scalar subset",
            ),
            (
                "lift-x86_64",
                "enable guest-x86_64",
                cfg!(feature = "guest-x86_64"),
                "scalar arithmetic, flags, memory and direct control flow",
            ),
            (
                "decode-aarch64",
                "enable guest-aarch64",
                cfg!(feature = "guest-aarch64"),
                "fixed-width little-endian A64 scalar decoding",
            ),
            (
                "lift-aarch64",
                "enable guest-aarch64",
                cfg!(feature = "guest-aarch64"),
                "scalar arithmetic, NZCV, branches and ordered memory",
            ),
            (
                "interpreter",
                "enable interp",
                cfg!(feature = "interp"),
                "bounded evaluation of verified scalar IR",
            ),
        ] {
            let record = if enabled {
                Capability::implemented(id, true, SupportLevel::Partial, detail)
                    .expect("implemented support level")
            } else {
                Capability::planned(id, false, feature)
            };
            capabilities
                .register(record)
                .expect("unique optional capability");
        }
        for (id, isa) in [
            ("raw-x86_64", GuestIsa::X86_64),
            ("raw-aarch64", GuestIsa::AArch64),
        ] {
            let raw = if guest_compiled(isa)
                && cfg!(any(feature = "interp", feature = "jit-cranelift"))
            {
                Capability::implemented(
                    id,
                    cfg!(feature = "interp") || jit_ready,
                    SupportLevel::Partial,
                    "bare scalar guest execution",
                )
                .expect("implemented support level")
            } else {
                Capability::planned(id, false, "enable the guest and an execution backend")
            };
            capabilities
                .register(raw)
                .expect("unique raw execution capability");
        }
        let jit = if cfg!(feature = "jit-cranelift") {
            Capability::implemented(
                "jit-cranelift",
                jit_available,
                SupportLevel::Partial,
                "scalar blocks with checked guest memory",
            )
            .expect("implemented support level")
        } else {
            Capability::planned("jit-cranelift", false, "enable jit-cranelift")
        };
        capabilities.register(jit).expect("unique JIT capability");
        let bridge = if cfg!(feature = "abi-bridge") {
            Capability::implemented(
                "abi-bridge",
                bridge_host_available(),
                SupportLevel::Partial,
                "registered fixed-signature native calls and synchronous callbacks",
            )
            .expect("implemented support level")
        } else {
            Capability::planned("abi-bridge", false, "enable abi-bridge")
        };
        capabilities
            .register(bridge)
            .expect("unique bridge capability");
        for (id, feature, available) in [
            (
                "loader-elf",
                "enable loader-elf",
                cfg!(feature = "loader-elf"),
            ),
            ("os-linux", "enable os-linux", cfg!(feature = "os-linux")),
            ("linux-elf", "enable linux-elf", cfg!(feature = "linux-elf")),
            ("loader-pe", "enable loader-pe", cfg!(feature = "loader-pe")),
            (
                "os-windows",
                "enable os-windows",
                cfg!(feature = "os-windows"),
            ),
            (
                "windows-pe",
                "enable windows-pe",
                cfg!(feature = "windows-pe"),
            ),
        ] {
            let record = if available {
                Capability::implemented(
                    id,
                    true,
                    SupportLevel::Partial,
                    "limited x86-64 executable profile",
                )
                .expect("implemented support")
            } else {
                Capability::planned(id, false, feature)
            };
            capabilities
                .register(record)
                .expect("unique Linux capability");
        }
        for (id, enabled, os) in [
            (
                "host-windows",
                cfg!(feature = "host-windows"),
                bifrost_core::HostOs::Windows,
            ),
            (
                "host-linux",
                cfg!(feature = "host-linux"),
                bifrost_core::HostOs::Linux,
            ),
            (
                "host-macos",
                cfg!(feature = "host-macos"),
                bifrost_core::HostOs::MacOs,
            ),
        ] {
            let record = if enabled {
                Capability::implemented(
                    id,
                    host.os == os && host.page_size.is_some(),
                    SupportLevel::Partial,
                    "host page-size probe",
                )
                .expect("implemented support level")
            } else {
                Capability::planned(id, false, "enable the matching host feature")
            };
            capabilities
                .register(record)
                .expect("unique host capability");
        }
        for id in ["native-execution", "c-abi"] {
            capabilities
                .register(Capability::planned(
                    id,
                    false,
                    "component unavailable in this build",
                ))
                .expect("unique built-in capability");
        }
        let macho = if cfg!(feature = "macho-inspect") {
            Capability::implemented(
                "macho-inspect",
                true,
                SupportLevel::Experimental,
                "thin Mach-O executable metadata inspection",
            )
            .expect("implemented support")
        } else {
            Capability::planned("macho-inspect", false, "enable macho-inspect")
        };
        capabilities
            .register(macho)
            .expect("unique Mach-O capability");
        Self {
            host,
            jit_ready,
            capabilities,
        }
    }
}

impl RuntimeProfile {
    pub const fn host(&self) -> HostTarget {
        self.host
    }

    /// Validate an execution request before any code is started.
    pub fn validate_execution(
        &self,
        guest: GuestTarget,
        mode: ExecMode,
    ) -> Result<(), UnsupportedExecutionMode> {
        self.select_execution(guest, mode).map(|_| ())
    }

    /// Resolves Auto to an available backend for either bare scalar guest profile.
    pub fn select_execution(
        &self,
        guest: GuestTarget,
        mode: ExecMode,
    ) -> Result<ExecMode, UnsupportedExecutionMode> {
        if supports_raw_profile(&guest) {
            match mode {
                ExecMode::Auto | ExecMode::Jit if self.jit_ready => return Ok(ExecMode::Jit),
                ExecMode::Auto | ExecMode::Interpreter if cfg!(feature = "interp") => {
                    return Ok(ExecMode::Interpreter);
                }
                _ => {}
            }
        }
        Err(UnsupportedExecutionMode {
            mode,
            guest,
            host: self.host,
        })
    }
}

impl CapabilityProvider for RuntimeProfile {
    fn capabilities(&self) -> &CapabilityRegistry {
        &self.capabilities
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedExecutionMode {
    pub mode: ExecMode,
    pub guest: GuestTarget,
    pub host: HostTarget,
}

impl fmt::Display for UnsupportedExecutionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} execution unavailable for {:?}/{:?} on {:?}/{:?}",
            self.mode,
            self.guest.os(),
            self.guest.isa(),
            self.host.os,
            self.host.isa
        )
    }
}

impl std::error::Error for UnsupportedExecutionMode {}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::{AbiId, CpuFeature};

    #[test]
    fn features_report_interpreter_and_raw_guest_support() {
        let profile = RuntimeProfile::default();
        let interp = profile.capabilities().get("interpreter").unwrap();
        assert_eq!(interp.compiled(), cfg!(feature = "interp"));
        assert_eq!(interp.host_available(), cfg!(feature = "interp"));
        for (id, isa) in [
            ("raw-x86_64", GuestIsa::X86_64),
            ("raw-aarch64", GuestIsa::AArch64),
        ] {
            let available = profile.capabilities().get(id).unwrap().host_available();
            assert_eq!(
                available,
                guest_compiled(isa) && (cfg!(feature = "interp") || profile.jit_ready)
            );
        }
        assert!(
            profile
                .capabilities()
                .get("ir-v1")
                .unwrap()
                .host_available()
        );
    }

    #[test]
    fn validates_only_the_enabled_bare_scalar_target() {
        let profile = RuntimeProfile::default();
        let guest = GuestTarget::new(
            GuestIsa::X86_64,
            GuestOs::None,
            AbiId::Bare,
            Default::default(),
        )
        .unwrap();
        for mode in [
            ExecMode::Auto,
            ExecMode::Native,
            ExecMode::Jit,
            ExecMode::Interpreter,
        ] {
            let result = profile.select_execution(guest.clone(), mode).ok();
            let expected = match mode {
                ExecMode::Auto | ExecMode::Jit
                    if profile.jit_ready && guest_compiled(guest.isa()) =>
                {
                    Some(ExecMode::Jit)
                }
                ExecMode::Auto | ExecMode::Interpreter
                    if cfg!(feature = "interp") && guest_compiled(guest.isa()) =>
                {
                    Some(ExecMode::Interpreter)
                }
                _ => None,
            };
            assert_eq!(result, expected);
        }
        let linux = GuestTarget::new(
            GuestIsa::X86_64,
            GuestOs::Linux,
            AbiId::LinuxSysvAmd64,
            Default::default(),
        )
        .unwrap();
        let sse = GuestTarget::new(
            GuestIsa::X86_64,
            GuestOs::None,
            AbiId::Bare,
            [CpuFeature::Sse2].into_iter().collect(),
        )
        .unwrap();
        for guest in [linux, sse] {
            assert!(profile.validate_execution(guest, ExecMode::Auto).is_err());
        }
    }
}
