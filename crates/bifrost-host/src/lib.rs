//! Host adapter identity and capability reporting.

#[allow(unsafe_code)]
pub mod jit;

use bifrost_core::{CapabilityProvider, HostTarget};

/// Host-neutral entry point. Implementations advertise only services they can provide.
pub trait HostServices: CapabilityProvider {
    fn target(&self) -> HostTarget;
}
