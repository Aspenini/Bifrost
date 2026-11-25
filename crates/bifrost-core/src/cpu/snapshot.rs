//! Versioned scalar and vector register snapshots for the modeled CPU state.

use super::{Arm64State, CpuState, GuestAddress, X86State};
use std::fmt;

const MAGIC: &[u8; 4] = b"BFRS";
const HEADER: usize = 8;
const ORIGINAL_VERSION: u8 = 1;
const ARM64_VERSION: u8 = 2;
const X86_BYTES: usize = 16 * 8 + 4 * 8 + 16 * 16 + 4;
const ARM64_BYTES: usize = 31 * 8 + 2 * 8 + 4 + 8 + 32 * 16 + 2 * 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SnapshotError {
    Header,
    Version(u8),
    Isa(u8),
    Length { expected: usize, actual: usize },
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header => f.write_str("invalid CPU snapshot header"),
            Self::Version(version) => write!(f, "unsupported CPU snapshot version {version}"),
            Self::Isa(isa) => write!(f, "unknown CPU snapshot ISA {isa}"),
            Self::Length { expected, actual } => {
                write!(
                    f,
                    "CPU snapshot contains {actual} bytes; expected {expected}"
                )
            }
        }
    }
}

impl std::error::Error for SnapshotError {}

impl CpuState {
    /// Encodes all modeled registers as little-endian bits.
    pub fn encode_snapshot(&self) -> Vec<u8> {
        let (version, isa, size) = match self {
            Self::X86_64(_) => (ORIGINAL_VERSION, 1, X86_BYTES),
            Self::AArch64(_) => (ARM64_VERSION, 2, ARM64_BYTES + 8),
        };
        let mut bytes = Vec::with_capacity(HEADER + size);
        bytes.extend_from_slice(MAGIC);
        bytes.extend([version, isa, 0, 0]);
        match self {
            Self::X86_64(state) => {
                bytes.extend(state.gpr.iter().flat_map(|value| value.to_le_bytes()));
                for value in [state.rip.0, state.rflags, state.fs_base.0, state.gs_base.0] {
                    bytes.extend(value.to_le_bytes());
                }
                bytes.extend(state.xmm.iter().flat_map(|value| value.to_le_bytes()));
                bytes.extend(state.mxcsr.to_le_bytes());
            }
            Self::AArch64(state) => {
                bytes.extend(state.x.iter().flat_map(|value| value.to_le_bytes()));
                bytes.extend(state.sp.0.to_le_bytes());
                bytes.extend(state.pc.0.to_le_bytes());
                bytes.extend(state.nzcv.to_le_bytes());
                bytes.extend(state.tpidr_el0.0.to_le_bytes());
                bytes.extend(state.v.iter().flat_map(|value| value.to_le_bytes()));
                bytes.extend(state.fpcr.to_le_bytes());
                bytes.extend(state.fpsr.to_le_bytes());
                bytes.extend(state.tpidrro_el0.0.to_le_bytes());
            }
        }
        bytes
    }

    /// Checks the format and restores register bits. Guest memory and thread state are separate.
    pub fn decode_snapshot(bytes: &[u8]) -> Result<Self, SnapshotError> {
        let Some((header, payload)) = bytes.split_at_checked(HEADER) else {
            return Err(SnapshotError::Header);
        };
        if &header[..4] != MAGIC || header[6..] != [0, 0] {
            return Err(SnapshotError::Header);
        }
        let version = header[4];
        let isa = header[5];
        if version != ORIGINAL_VERSION && !(version == ARM64_VERSION && isa == 2) {
            return Err(SnapshotError::Version(header[4]));
        }
        let expected = HEADER
            + match isa {
                1 => X86_BYTES,
                2 => ARM64_BYTES + if version == ARM64_VERSION { 8 } else { 0 },
                _ => return Err(SnapshotError::Isa(isa)),
            };
        if bytes.len() != expected {
            return Err(SnapshotError::Length {
                expected,
                actual: bytes.len(),
            });
        }
        let mut read = RegisterBytes(payload);
        if isa == 1 {
            Ok(Self::X86_64(Box::new(X86State {
                gpr: std::array::from_fn(|_| read.u64()),
                rip: GuestAddress(read.u64()),
                rflags: read.u64(),
                fs_base: GuestAddress(read.u64()),
                gs_base: GuestAddress(read.u64()),
                xmm: std::array::from_fn(|_| read.u128()),
                mxcsr: read.u32(),
            })))
        } else {
            Ok(Self::AArch64(Box::new(Arm64State {
                x: std::array::from_fn(|_| read.u64()),
                sp: GuestAddress(read.u64()),
                pc: GuestAddress(read.u64()),
                nzcv: read.u32(),
                tpidr_el0: GuestAddress(read.u64()),
                v: std::array::from_fn(|_| read.u128()),
                fpcr: read.u32(),
                fpsr: read.u32(),
                tpidrro_el0: if version == ARM64_VERSION {
                    GuestAddress(read.u64())
                } else {
                    GuestAddress(0)
                },
            })))
        }
    }
}

struct RegisterBytes<'a>(&'a [u8]);

impl RegisterBytes<'_> {
    // The decoder checks the exact payload size for the selected version and ISA first.
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let (data, remaining) = self.0.split_at(N);
        self.0 = remaining;
        data.try_into().expect("register width")
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }
    fn u128(&mut self) -> u128 {
        u128::from_le_bytes(self.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_modeled_register() {
        let x86 = CpuState::X86_64(Box::new(X86State {
            gpr: std::array::from_fn(|index| 100 + index as u64),
            rip: GuestAddress(0x1020),
            rflags: 0x202,
            fs_base: GuestAddress(0x1111),
            gs_base: GuestAddress(0x2222),
            xmm: std::array::from_fn(|index| (1_u128 << 100) + index as u128),
            mxcsr: 0x1f80,
        }));
        let arm = CpuState::AArch64(Box::new(Arm64State {
            x: std::array::from_fn(|index| 1000 + index as u64),
            sp: GuestAddress(0x3000),
            pc: GuestAddress(0x4000),
            nzcv: 0xb0000000,
            tpidr_el0: GuestAddress(0x5000),
            tpidrro_el0: GuestAddress(0x6000),
            v: std::array::from_fn(|index| (1_u128 << 124) + index as u128),
            fpcr: 0x400000,
            fpsr: 0x11,
        }));
        for state in [x86, arm] {
            let bytes = state.encode_snapshot();
            assert_eq!(CpuState::decode_snapshot(&bytes), Ok(state));
            for prefix in 0..bytes.len() {
                assert!(CpuState::decode_snapshot(&bytes[..prefix]).is_err());
            }
        }
    }

    #[test]
    fn restores_version_one_arm64_snapshots() {
        let state = CpuState::AArch64(Box::new(Arm64State {
            tpidr_el0: GuestAddress(0x1234),
            ..Default::default()
        }));
        let mut bytes = state.encode_snapshot();
        bytes[4] = ORIGINAL_VERSION;
        bytes.truncate(HEADER + ARM64_BYTES);
        assert_eq!(CpuState::decode_snapshot(&bytes), Ok(state));
    }

    #[test]
    fn rejects_unknown_versions_isa_tags_and_trailing_data() {
        let mut bytes = CpuState::X86_64(Box::default()).encode_snapshot();
        bytes[4] = 2;
        assert_eq!(
            CpuState::decode_snapshot(&bytes),
            Err(SnapshotError::Version(2))
        );
        bytes[4] = 1;
        bytes[5] = 99;
        assert_eq!(
            CpuState::decode_snapshot(&bytes),
            Err(SnapshotError::Isa(99))
        );
        bytes[5] = 1;
        bytes.push(0);
        assert!(matches!(
            CpuState::decode_snapshot(&bytes),
            Err(SnapshotError::Length { .. })
        ));
    }
}
