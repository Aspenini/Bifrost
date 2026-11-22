//! Guest call-frame descriptors for four little-endian, 64-bit ABIs.

use bifrost_core::{
    AbiId, CpuState, GuestAddress, GuestIsa, GuestMemory, GuestMemoryRead, MemoryError,
};
use std::fmt;

mod layout;
pub use layout::{Argument, ArgumentLocation, CallLayout, Pair64, ResultLocation};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArgumentRegisters {
    Independent,
    SharedPositions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnAddress {
    Stack,
    LinkRegister(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VariadicRule {
    SysVVectorCount,
    WindowsX64FloatDuplication,
    Aapcs64SaveAreas,
    WindowsArm64IntegerConvention,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnwindRule {
    Dwarf,
    WindowsX64,
    WindowsArm64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairPassing {
    IntegerSlots,
    IndirectAligned16,
}

/// Register numbers use architectural encoding order (RAX=0 on x86-64, X0=0 on ARM64).
#[derive(Debug, Eq, PartialEq)]
pub struct AbiDescriptor {
    id: AbiId,
    pub isa: GuestIsa,
    pub argument_registers: ArgumentRegisters,
    pub integer_args: &'static [u8],
    pub float_args: &'static [u8],
    pub integer_returns: &'static [u8],
    pub float_return: u8,
    pub stack_alignment: u8,
    pub shadow_bytes: u8,
    pub red_zone_bytes: u8,
    pub return_address: ReturnAddress,
    pub preserved_integer: &'static [u8],
    pub preserved_vector: &'static [u8],
    pub preserved_vector_bits: u8,
    pub platform_register: Option<u8>,
    pub variadic: VariadicRule,
    pub unwind: UnwindRule,
    pub pair_passing: PairPassing,
}

const ARM_ARGS: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7];
const ARM_PRESERVED: &[u8] = &[19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29];
const ARM_V_PRESERVED: &[u8] = &[8, 9, 10, 11, 12, 13, 14, 15];

const SYSV: AbiDescriptor = AbiDescriptor {
    id: AbiId::LinuxSysvAmd64,
    isa: GuestIsa::X86_64,
    argument_registers: ArgumentRegisters::Independent,
    integer_args: &[7, 6, 2, 1, 8, 9],
    float_args: ARM_ARGS,
    integer_returns: &[0, 2],
    float_return: 0,
    stack_alignment: 16,
    shadow_bytes: 0,
    red_zone_bytes: 128,
    return_address: ReturnAddress::Stack,
    preserved_integer: &[3, 5, 12, 13, 14, 15],
    preserved_vector: &[],
    preserved_vector_bits: 0,
    platform_register: None,
    variadic: VariadicRule::SysVVectorCount,
    unwind: UnwindRule::Dwarf,
    pair_passing: PairPassing::IntegerSlots,
};

const WIN_X64: AbiDescriptor = AbiDescriptor {
    id: AbiId::WindowsX64,
    isa: GuestIsa::X86_64,
    argument_registers: ArgumentRegisters::SharedPositions,
    integer_args: &[1, 2, 8, 9],
    float_args: &[0, 1, 2, 3],
    integer_returns: &[0],
    float_return: 0,
    stack_alignment: 16,
    shadow_bytes: 32,
    red_zone_bytes: 0,
    return_address: ReturnAddress::Stack,
    preserved_integer: &[3, 5, 6, 7, 12, 13, 14, 15],
    preserved_vector: &[6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    preserved_vector_bits: 128,
    platform_register: None,
    variadic: VariadicRule::WindowsX64FloatDuplication,
    unwind: UnwindRule::WindowsX64,
    pair_passing: PairPassing::IndirectAligned16,
};

const AAPCS: AbiDescriptor = AbiDescriptor {
    id: AbiId::LinuxAapcs64,
    isa: GuestIsa::AArch64,
    argument_registers: ArgumentRegisters::Independent,
    integer_args: ARM_ARGS,
    float_args: ARM_ARGS,
    integer_returns: &[0, 1],
    float_return: 0,
    stack_alignment: 16,
    shadow_bytes: 0,
    red_zone_bytes: 0,
    return_address: ReturnAddress::LinkRegister(30),
    preserved_integer: ARM_PRESERVED,
    preserved_vector: ARM_V_PRESERVED,
    preserved_vector_bits: 64,
    platform_register: None,
    variadic: VariadicRule::Aapcs64SaveAreas,
    unwind: UnwindRule::Dwarf,
    pair_passing: PairPassing::IntegerSlots,
};

const WIN_ARM64: AbiDescriptor = AbiDescriptor {
    id: AbiId::WindowsArm64,
    platform_register: Some(18),
    red_zone_bytes: 16,
    variadic: VariadicRule::WindowsArm64IntegerConvention,
    unwind: UnwindRule::WindowsArm64,
    ..AAPCS
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scalar {
    U64,
    I64,
    F64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScalarValue {
    U64(u64),
    I64(i64),
    F64(f64),
}

impl ScalarValue {
    pub const fn kind(self) -> Scalar {
        match self {
            Self::U64(_) => Scalar::U64,
            Self::I64(_) => Scalar::I64,
            Self::F64(_) => Scalar::F64,
        }
    }

    pub const fn bits(self) -> u64 {
        match self {
            Self::U64(bits) => bits,
            Self::I64(bits) => bits as u64,
            Self::F64(value) => value.to_bits(),
        }
    }

    pub const fn from_bits(kind: Scalar, bits: u64) -> Self {
        match kind {
            Scalar::U64 => Self::U64(bits),
            Scalar::I64 => Self::I64(bits as i64),
            Scalar::F64 => Self::F64(f64::from_bits(bits)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Location {
    Integer(u8),
    Float(u8),
    Stack(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AbiError {
    UnsupportedAbi(AbiId),
    IsaMismatch {
        expected: GuestIsa,
        actual: GuestIsa,
    },
    MisalignedStack(GuestAddress),
    InvalidAddress(GuestAddress),
    StackOffsetOverflow,
    InvalidRegister(Location),
    Memory(MemoryError),
}

impl fmt::Display for AbiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedAbi(id) => write!(f, "unsupported call ABI {id:?}"),
            Self::IsaMismatch { expected, actual } => {
                write!(f, "call ABI requires {expected:?}, received {actual:?}")
            }
            Self::MisalignedStack(sp) => write!(f, "misaligned guest call stack at {:#x}", sp.0),
            Self::InvalidAddress(address) => {
                write!(f, "invalid guest ABI address {:#x}", address.0)
            }
            Self::StackOffsetOverflow => f.write_str("guest ABI stack layout overflow"),
            Self::InvalidRegister(location) => write!(f, "invalid guest ABI register {location:?}"),
            Self::Memory(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for AbiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Memory(error) => Some(error),
            _ => None,
        }
    }
}
impl From<MemoryError> for AbiError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}

/// Restores the guest control state as if the bridged function returned normally.
#[derive(Clone, Copy, Debug)]
pub struct CallReturn {
    pc: GuestAddress,
    sp: GuestAddress,
}

impl CallReturn {
    pub fn apply(self, cpu: &mut CpuState) {
        cpu.set_pc(self.pc);
        match cpu {
            CpuState::X86_64(state) => state.gpr[4] = self.sp.0,
            CpuState::AArch64(state) => state.sp = self.sp,
            _ => {}
        }
    }
}

impl AbiDescriptor {
    pub const fn for_id(id: AbiId) -> Result<&'static Self, AbiError> {
        match id {
            AbiId::LinuxSysvAmd64 => Ok(&SYSV),
            AbiId::WindowsX64 => Ok(&WIN_X64),
            AbiId::LinuxAapcs64 => Ok(&AAPCS),
            AbiId::WindowsArm64 => Ok(&WIN_ARM64),
            other => Err(AbiError::UnsupportedAbi(other)),
        }
    }

    pub const fn id(&self) -> AbiId {
        self.id
    }

    /// The current process's compiler-provided native C ABI, when described here.
    pub const fn native() -> Option<&'static Self> {
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(&WIN_X64)
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(&SYSV)
        } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            Some(&AAPCS)
        } else {
            None
        }
    }

    pub const fn pointer_bytes(&self) -> u8 {
        8
    }

    pub const fn entry_stack_bias(&self) -> u8 {
        match self.return_address {
            ReturnAddress::Stack => 8,
            ReturnAddress::LinkRegister(_) => 0,
        }
    }

    pub(crate) fn cursor(&self) -> Cursor<'_> {
        Cursor {
            abi: self,
            integer: 0,
            float: 0,
            positions: 0,
            stack: u32::from(self.entry_stack_bias()) + u32::from(self.shadow_bytes),
        }
    }

    pub fn argument_locations(&self, signature: &[Scalar]) -> Result<Vec<Location>, AbiError> {
        let mut cursor = self.cursor();
        signature.iter().map(|&kind| cursor.scalar(kind)).collect()
    }

    pub fn scalar_return(&self, kind: Scalar) -> Location {
        if kind == Scalar::F64 {
            Location::Float(self.float_return)
        } else {
            Location::Integer(self.integer_returns[0])
        }
    }

    pub fn check_state(&self, cpu: &CpuState) -> Result<(), AbiError> {
        let actual = cpu.isa();
        if actual != self.isa {
            return Err(AbiError::IsaMismatch {
                expected: self.isa,
                actual,
            });
        }
        let sp = cpu.sp();
        if sp.0 % u64::from(self.stack_alignment) != u64::from(self.entry_stack_bias()) {
            return Err(AbiError::MisalignedStack(sp));
        }
        self.validate_span(sp, 1)
    }

    pub fn validate_span(&self, address: GuestAddress, len: usize) -> Result<(), AbiError> {
        let last = address
            .checked_add(len.saturating_sub(1) as u64)
            .ok_or(AbiError::InvalidAddress(address))?;
        if self.isa == GuestIsa::X86_64
            && (!address.is_canonical_48()
                || !last.is_canonical_48()
                || ((address.0 ^ last.0) >> 47) != 0)
        {
            return Err(AbiError::InvalidAddress(address));
        }
        Ok(())
    }

    pub fn validate_code_address(&self, address: GuestAddress) -> Result<(), AbiError> {
        self.validate_span(address, if self.isa == GuestIsa::AArch64 { 4 } else { 1 })?;
        if self.isa == GuestIsa::AArch64 && !address.0.is_multiple_of(4) {
            return Err(AbiError::InvalidAddress(address));
        }
        Ok(())
    }

    pub fn capture_return(
        &self,
        cpu: &CpuState,
        memory: &dyn GuestMemoryRead,
    ) -> Result<CallReturn, AbiError> {
        self.check_state(cpu)?;
        let sp = cpu.sp();
        let (location, next_sp) = match self.return_address {
            ReturnAddress::Stack => (
                Location::Stack(0),
                sp.checked_add(8).ok_or(AbiError::InvalidAddress(sp))?,
            ),
            ReturnAddress::LinkRegister(reg) => (Location::Integer(reg), sp),
        };
        let pc = GuestAddress(self.read_word(cpu, memory, location)?);
        self.validate_code_address(pc)?;
        self.validate_span(next_sp, 1)?;
        Ok(CallReturn { pc, sp: next_sp })
    }

    pub fn read_word(
        &self,
        cpu: &CpuState,
        memory: &dyn GuestMemoryRead,
        location: Location,
    ) -> Result<u64, AbiError> {
        self.check_state(cpu)?;
        if let Location::Stack(offset) = location {
            let mut bytes = [0; 8];
            memory.read(self.stack_word(cpu.sp(), offset)?, &mut bytes)?;
            Ok(u64::from_le_bytes(bytes))
        } else {
            self.read_result_register(cpu, location)
        }
    }

    /// Reads a returned register without imposing callee-entry stack alignment.
    pub fn read_result_register(
        &self,
        cpu: &CpuState,
        location: Location,
    ) -> Result<u64, AbiError> {
        let actual = cpu.isa();
        if actual != self.isa {
            return Err(AbiError::IsaMismatch {
                expected: self.isa,
                actual,
            });
        }
        let result = match (cpu, location) {
            (CpuState::X86_64(state), Location::Integer(reg)) => {
                state.gpr.get(usize::from(reg)).copied()
            }
            (CpuState::AArch64(state), Location::Integer(reg)) => {
                state.x.get(usize::from(reg)).copied()
            }
            (CpuState::X86_64(state), Location::Float(reg)) => {
                state.xmm.get(usize::from(reg)).map(|&bits| bits as u64)
            }
            (CpuState::AArch64(state), Location::Float(reg)) => {
                state.v.get(usize::from(reg)).map(|&bits| bits as u64)
            }
            _ => None,
        };
        result.ok_or(AbiError::InvalidRegister(location))
    }

    pub fn write_word(
        &self,
        cpu: &mut CpuState,
        memory: &mut dyn GuestMemory,
        location: Location,
        bits: u64,
    ) -> Result<(), AbiError> {
        self.check_state(cpu)?;
        match (&mut *cpu, location) {
            (CpuState::X86_64(state), Location::Integer(reg)) => {
                *state
                    .gpr
                    .get_mut(usize::from(reg))
                    .ok_or(AbiError::InvalidRegister(location))? = bits
            }
            (CpuState::AArch64(state), Location::Integer(reg)) => {
                *state
                    .x
                    .get_mut(usize::from(reg))
                    .ok_or(AbiError::InvalidRegister(location))? = bits
            }
            (CpuState::X86_64(state), Location::Float(reg)) => {
                *state
                    .xmm
                    .get_mut(usize::from(reg))
                    .ok_or(AbiError::InvalidRegister(location))? = bits.into()
            }
            (CpuState::AArch64(state), Location::Float(reg)) => {
                *state
                    .v
                    .get_mut(usize::from(reg))
                    .ok_or(AbiError::InvalidRegister(location))? = bits.into()
            }
            (_, Location::Stack(offset)) => {
                memory.write(self.stack_word(cpu.sp(), offset)?, &bits.to_le_bytes())?
            }
            _ => return Err(AbiError::InvalidRegister(location)),
        }
        Ok(())
    }

    fn stack_word(&self, sp: GuestAddress, offset: u32) -> Result<GuestAddress, AbiError> {
        let address = sp
            .checked_add(u64::from(offset))
            .ok_or(AbiError::InvalidAddress(sp))?;
        self.validate_span(address, 8)?;
        Ok(address)
    }
}

pub(crate) struct Cursor<'a> {
    abi: &'a AbiDescriptor,
    integer: usize,
    float: usize,
    positions: usize,
    stack: u32,
}

impl Cursor<'_> {
    pub(crate) fn scalar(&mut self, kind: Scalar) -> Result<Location, AbiError> {
        let (bank, index) = if kind == Scalar::F64 {
            (self.abi.float_args, &mut self.float)
        } else {
            (self.abi.integer_args, &mut self.integer)
        };
        let slot = if self.abi.argument_registers == ArgumentRegisters::SharedPositions {
            let slot = self.positions;
            self.positions = self.positions.saturating_add(1);
            slot
        } else {
            let slot = *index;
            *index = index.saturating_add(1);
            slot
        };
        match bank.get(slot) {
            Some(&reg) if kind == Scalar::F64 => Ok(Location::Float(reg)),
            Some(&reg) => Ok(Location::Integer(reg)),
            None => self.stack_slot(),
        }
    }

    pub(crate) fn stack_slot(&mut self) -> Result<Location, AbiError> {
        let offset = self.stack;
        self.stack = offset.checked_add(8).ok_or(AbiError::StackOffsetOverflow)?;
        Ok(Location::Stack(offset))
    }
}
