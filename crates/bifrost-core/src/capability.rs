use std::collections::{BTreeMap, btree_map::Entry};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SupportLevel {
    Unavailable,
    Experimental,
    Partial,
    Tested,
}

/// Compilation, availability and verification are reported separately for the current host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityStatus {
    NotCompiled,
    Reserved,
    HostUnavailable,
    Available(SupportLevel),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capability {
    id: &'static str,
    status: CapabilityStatus,
    detail: &'static str,
}

impl Capability {
    /// Constructs a record for an unimplemented component.
    pub const fn planned(id: &'static str, compiled: bool, detail: &'static str) -> Self {
        let status = if compiled {
            CapabilityStatus::Reserved
        } else {
            CapabilityStatus::NotCompiled
        };
        Self { id, status, detail }
    }

    pub const fn implemented(
        id: &'static str,
        available: bool,
        level: SupportLevel,
        detail: &'static str,
    ) -> Option<Self> {
        if matches!(level, SupportLevel::Unavailable) {
            return None;
        }
        let status = if available {
            CapabilityStatus::Available(level)
        } else {
            CapabilityStatus::HostUnavailable
        };
        Some(Self { id, status, detail })
    }

    pub const fn id(&self) -> &'static str {
        self.id
    }
    pub const fn detail(&self) -> &'static str {
        self.detail
    }
    pub const fn level(&self) -> SupportLevel {
        match self.status {
            CapabilityStatus::Available(level) => level,
            _ => SupportLevel::Unavailable,
        }
    }
    pub const fn compiled(&self) -> bool {
        !matches!(self.status, CapabilityStatus::NotCompiled)
    }
    pub const fn implemented_on_any_host(&self) -> bool {
        matches!(
            self.status,
            CapabilityStatus::Available(_) | CapabilityStatus::HostUnavailable
        )
    }
    pub const fn host_available(&self) -> bool {
        matches!(self.status, CapabilityStatus::Available(_))
    }
    pub const fn validated(&self) -> bool {
        matches!(
            self.status,
            CapabilityStatus::Available(SupportLevel::Tested)
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct CapabilityRegistry(BTreeMap<&'static str, Capability>);

impl CapabilityRegistry {
    pub fn register(&mut self, capability: Capability) -> Result<(), DuplicateCapability> {
        match self.0.entry(capability.id()) {
            Entry::Vacant(entry) => {
                entry.insert(capability);
                Ok(())
            }
            Entry::Occupied(_) => Err(DuplicateCapability(capability.id())),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Capability> {
        self.0.get(id)
    }
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Capability> {
        self.0.values()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DuplicateCapability(pub &'static str);

impl fmt::Display for DuplicateCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "capability {} is already registered", self.0)
    }
}

impl std::error::Error for DuplicateCapability {}

/// Providers describe capabilities through shared borrows. Parallel callers may require `Sync`.
pub trait CapabilityProvider {
    fn capabilities(&self) -> &CapabilityRegistry;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolls_up_status_without_conflating_implementation_and_validation() {
        let mut registry = CapabilityRegistry::default();
        let reserved = Capability::planned("jit", true, "reserved");
        let skipped = Capability::planned("interp", false, "feature disabled");
        let unavailable =
            Capability::implemented("native", false, SupportLevel::Tested, "wrong host").unwrap();
        let tested =
            Capability::implemented("ir", true, SupportLevel::Tested, "fixtures pass").unwrap();
        let partial =
            Capability::implemented("cpu", true, SupportLevel::Partial, "subset").unwrap();
        for record in [tested, partial, unavailable, reserved, skipped] {
            registry.register(record).unwrap();
        }
        assert_eq!(registry.register(tested), Err(DuplicateCapability("ir")));
        assert_eq!(
            registry.iter().map(Capability::id).collect::<Vec<_>>(),
            ["cpu", "interp", "ir", "jit", "native"]
        );
        assert!(tested.validated() && tested.host_available() && tested.implemented_on_any_host());
        assert!(partial.host_available() && !partial.validated());
        assert!(
            unavailable.implemented_on_any_host()
                && !unavailable.host_available()
                && !unavailable.validated()
        );
        assert!(reserved.compiled() && !reserved.implemented_on_any_host());
        assert!(!skipped.compiled());
        assert_eq!(registry.get("jit"), Some(&reserved));
    }
}
