use crate::{AbiId, GuestAddress, GuestFault, GuestIsa, GuestOs, GuestThreadId};
use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum CpuFeature {
    Sse2,
    Avx,
    Avx2,
    Neon,
    Lse,
}

impl CpuFeature {
    pub const fn isa(self) -> GuestIsa {
        match self {
            Self::Sse2 | Self::Avx | Self::Avx2 => GuestIsa::X86_64,
            Self::Neon | Self::Lse => GuestIsa::AArch64,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CpuFeatureSet(BTreeSet<CpuFeature>);

impl CpuFeatureSet {
    pub fn contains(&self, feature: CpuFeature) -> bool {
        self.0.contains(&feature)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = CpuFeature> + '_ {
        self.0.iter().copied()
    }
}

impl FromIterator<CpuFeature> for CpuFeatureSet {
    fn from_iter<T: IntoIterator<Item = CpuFeature>>(features: T) -> Self {
        Self(features.into_iter().collect())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuestTarget {
    isa: GuestIsa,
    os: GuestOs,
    abi: AbiId,
    features: CpuFeatureSet,
}

impl GuestTarget {
    pub fn new(
        isa: GuestIsa,
        os: GuestOs,
        abi: AbiId,
        features: CpuFeatureSet,
    ) -> Result<Self, TargetError> {
        if !abi.matches(isa, os) {
            return Err(TargetError::AbiMismatch { isa, os, abi });
        }
        if let Some(feature) = features.iter().find(|feature| feature.isa() != isa) {
            return Err(TargetError::FeatureIsaMismatch { isa, feature });
        }
        for (feature, required) in [
            (CpuFeature::Avx2, CpuFeature::Avx),
            (CpuFeature::Avx, CpuFeature::Sse2),
        ] {
            if features.contains(feature) && !features.contains(required) {
                return Err(TargetError::MissingPrerequisite { feature, required });
            }
        }
        Ok(Self {
            isa,
            os,
            abi,
            features,
        })
    }

    pub const fn isa(&self) -> GuestIsa {
        self.isa
    }
    pub const fn os(&self) -> GuestOs {
        self.os
    }
    pub const fn abi(&self) -> AbiId {
        self.abi
    }
    pub const fn features(&self) -> &CpuFeatureSet {
        &self.features
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TargetError {
    AbiMismatch {
        isa: GuestIsa,
        os: GuestOs,
        abi: AbiId,
    },
    FeatureIsaMismatch {
        isa: GuestIsa,
        feature: CpuFeature,
    },
    MissingPrerequisite {
        feature: CpuFeature,
        required: CpuFeature,
    },
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AbiMismatch { isa, os, abi } => {
                write!(f, "ABI {abi:?} does not describe {os:?}/{isa:?}")
            }
            Self::FeatureIsaMismatch { isa, feature } => {
                write!(f, "{feature:?} is not a {isa:?} feature")
            }
            Self::MissingPrerequisite { feature, required } => {
                write!(f, "{feature:?} requires {required:?}")
            }
        }
    }
}

impl std::error::Error for TargetError {}

/// Architectural encoding order, with RSP at index 4. Only full-width registers are modeled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct X86State {
    pub gpr: [u64; 16],
    pub rip: GuestAddress,
    pub rflags: u64,
    pub fs_base: GuestAddress,
    pub gs_base: GuestAddress,
    pub xmm: [u128; 16],
    pub mxcsr: u32,
}

impl Default for X86State {
    fn default() -> Self {
        Self {
            gpr: [0; 16],
            rip: GuestAddress(0),
            rflags: 2,
            fs_base: GuestAddress(0),
            gs_base: GuestAddress(0),
            xmm: [0; 16],
            mxcsr: 0x1f80,
        }
    }
}

/// X0–X30, separate SP, and NZCV in the architectural bit positions (31–28).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Arm64State {
    pub x: [u64; 31],
    pub sp: GuestAddress,
    pub pc: GuestAddress,
    pub nzcv: u32,
    pub tpidr_el0: GuestAddress,
    pub v: [u128; 32],
    pub fpcr: u32,
    pub fpsr: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CpuState {
    X86_64(Box<X86State>),
    AArch64(Box<Arm64State>),
}

impl CpuState {
    pub const fn isa(&self) -> GuestIsa {
        match self {
            Self::X86_64(_) => GuestIsa::X86_64,
            Self::AArch64(_) => GuestIsa::AArch64,
        }
    }

    pub const fn pc(&self) -> GuestAddress {
        match self {
            Self::X86_64(state) => state.rip,
            Self::AArch64(state) => state.pc,
        }
    }

    pub fn set_pc(&mut self, pc: GuestAddress) {
        match self {
            Self::X86_64(state) => state.rip = pc,
            Self::AArch64(state) => state.pc = pc,
        }
    }

    pub const fn sp(&self) -> GuestAddress {
        match self {
            Self::X86_64(state) => GuestAddress(state.gpr[4]),
            Self::AArch64(state) => state.sp,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuestThread {
    pub id: GuestThreadId,
    pub cpu: CpuState,
    pub pending_fault: Option<GuestFault>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_abi_and_feature_prerequisites() {
        assert!(matches!(
            GuestTarget::new(
                GuestIsa::AArch64,
                GuestOs::Linux,
                AbiId::LinuxSysvAmd64,
                CpuFeatureSet::default()
            ),
            Err(TargetError::AbiMismatch { .. })
        ));
        assert!(matches!(
            GuestTarget::new(
                GuestIsa::X86_64,
                GuestOs::None,
                AbiId::Bare,
                [CpuFeature::Neon].into_iter().collect()
            ),
            Err(TargetError::FeatureIsaMismatch { .. })
        ));
        assert!(matches!(
            GuestTarget::new(
                GuestIsa::X86_64,
                GuestOs::Linux,
                AbiId::LinuxSysvAmd64,
                [CpuFeature::Avx2].into_iter().collect()
            ),
            Err(TargetError::MissingPrerequisite { .. })
        ));
        let features = [
            CpuFeature::Avx2,
            CpuFeature::Sse2,
            CpuFeature::Avx,
            CpuFeature::Avx2,
        ]
        .into_iter()
        .collect();
        let target = GuestTarget::new(
            GuestIsa::X86_64,
            GuestOs::Linux,
            AbiId::LinuxSysvAmd64,
            features,
        )
        .unwrap();
        assert_eq!(
            target.features().iter().collect::<Vec<_>>(),
            [CpuFeature::Sse2, CpuFeature::Avx, CpuFeature::Avx2]
        );
    }

    #[test]
    fn exposes_architectural_stack_and_program_counters() {
        let mut x86 = X86State {
            rip: GuestAddress(0x1000),
            ..X86State::default()
        };
        x86.gpr[4] = 0x8000;
        let arm = Arm64State {
            sp: GuestAddress(0x8000),
            pc: GuestAddress(0x1000),
            ..Arm64State::default()
        };
        for state in [
            CpuState::X86_64(Box::new(x86)),
            CpuState::AArch64(Box::new(arm)),
        ] {
            assert_eq!(state.pc(), GuestAddress(0x1000));
            assert_eq!(state.sp(), GuestAddress(0x8000));
        }
    }
}
