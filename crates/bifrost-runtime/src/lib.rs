//! High-level capability discovery for the components selected by Cargo features.

use bifrost_core::{
    Capability, CapabilityProvider, CapabilityRegistry, ExecMode, GuestIsa, GuestOs, GuestTarget,
    HostTarget, SupportLevel,
};
use std::fmt;

pub use bifrost_core as core;
pub use bifrost_ir as ir;
pub use bifrost_memory as memory;

#[cfg(all(feature = "guest-x86_64", feature = "interp"))]
pub mod x86;

const RAW_X86: bool = cfg!(all(feature = "guest-x86_64", feature = "interp"));

#[derive(Clone, Debug)]
pub struct RuntimeProfile {
    host: HostTarget,
    capabilities: CapabilityRegistry,
}

impl Default for RuntimeProfile {
    fn default() -> Self {
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
                "interpreter",
                "enable interp",
                cfg!(feature = "interp"),
                "bounded evaluation of verified scalar IR",
            ),
            (
                "raw-x86_64",
                "enable guest-x86_64 and interp",
                RAW_X86,
                "bare x86-64 interpretation for the declared instruction subset",
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
        for (id, compiled) in [
            ("decode-aarch64", cfg!(feature = "guest-aarch64")),
            ("jit-cranelift", cfg!(feature = "jit-cranelift")),
            ("loader-elf", cfg!(feature = "loader-elf")),
            ("loader-pe", cfg!(feature = "loader-pe")),
            ("os-linux", cfg!(feature = "os-linux")),
            ("os-windows", cfg!(feature = "os-windows")),
            ("host-windows", cfg!(feature = "host-windows")),
            ("host-linux", cfg!(feature = "host-linux")),
            ("native-execution", false),
            ("c-abi", false),
        ] {
            let detail = if compiled {
                "crate boundary reserved"
            } else {
                "component unavailable in this build"
            };
            capabilities
                .register(Capability::planned(id, compiled, detail))
                .expect("unique built-in capability");
        }
        Self {
            host: HostTarget::current(),
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
        if RAW_X86
            && guest.isa() == GuestIsa::X86_64
            && guest.os() == GuestOs::None
            && guest.features().iter().len() == 0
            && matches!(mode, ExecMode::Auto | ExecMode::Interpreter)
        {
            return Ok(());
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
        assert_eq!(
            profile
                .capabilities()
                .get("raw-x86_64")
                .unwrap()
                .host_available(),
            RAW_X86
        );
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
            let result = profile.validate_execution(guest.clone(), mode);
            assert_eq!(
                result.is_ok(),
                RAW_X86 && matches!(mode, ExecMode::Auto | ExecMode::Interpreter)
            );
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
