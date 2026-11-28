//! Bifröst IR v1, a typed, block-local SSA representation of guest effects.

mod verify;

use bifrost_core::{
    CpuFeature, CpuFeatureSet, FaultClass, GuestAddress, GuestIsa, GuestMemoryRead, GuestTarget,
};
use std::fmt;
pub use verify::{VerifiedIrBlock, VerifiedIrFunction, VerifyError, VerifyErrorKind};

pub const IR_VERSION: u32 = 1;

/// A guest frontend borrows memory for decoding and emits unverified IR for the caller.
/// It must fetch instructions through the execute-permission path of guest memory.
pub trait InstructionLifter {
    type Error: std::error::Error + Send + Sync + 'static;

    fn guest_isa(&self) -> GuestIsa;
    fn lift_block(
        &self,
        memory: &dyn GuestMemoryRead,
        pc: GuestAddress,
        features: &CpuFeatureSet,
    ) -> Result<IrFunction, Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Type {
    I1,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    V64,
    V128,
    /// A memory layout for two vectors. It cannot be used as an SSA value.
    V128Pair,
    GuestAddress,
}

impl Type {
    pub const fn bit_width(self) -> u32 {
        match self {
            Self::I1 => 1,
            Self::I8 => 8,
            Self::I16 => 16,
            Self::I32 | Self::F32 => 32,
            Self::I64 | Self::F64 | Self::V64 | Self::GuestAddress => 64,
            Self::I128 | Self::V128 => 128,
            Self::V128Pair => 256,
        }
    }

    pub const fn is_integer(self) -> bool {
        matches!(
            self,
            Self::I1 | Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::I128
        )
    }

    /// Byte width of values permitted in guest memory operations.
    pub const fn storage_bytes(self) -> Option<u8> {
        match self {
            Self::I1 => None,
            _ => Some((self.bit_width() / 8) as u8),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ValueId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub value: ValueId,
    pub ty: Type,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodeSpan {
    pub start: GuestAddress,
    pub len: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum X86Reg {
    Gpr(u8),
    Xmm(u8),
    Rflags,
    FsBase,
    GsBase,
    Mxcsr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arm64Reg {
    X(u8),
    V(u8),
    Sp,
    Nzcv,
    TpidrEl0,
    TpidrroEl0,
    Fpcr,
    Fpsr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Register {
    X86(X86Reg),
    Arm64(Arm64Reg),
}

impl Register {
    fn ty_for(self, isa: GuestIsa) -> Option<Type> {
        match (isa, self) {
            (GuestIsa::X86_64, Self::X86(X86Reg::Gpr(i))) if i < 16 => Some(Type::I64),
            (GuestIsa::X86_64, Self::X86(X86Reg::Xmm(i))) if i < 16 => Some(Type::V128),
            (GuestIsa::X86_64, Self::X86(X86Reg::Rflags)) => Some(Type::I64),
            (GuestIsa::X86_64, Self::X86(X86Reg::FsBase | X86Reg::GsBase)) => {
                Some(Type::GuestAddress)
            }
            (GuestIsa::X86_64, Self::X86(X86Reg::Mxcsr)) => Some(Type::I32),
            (GuestIsa::AArch64, Self::Arm64(Arm64Reg::X(i))) if i < 31 => Some(Type::I64),
            (GuestIsa::AArch64, Self::Arm64(Arm64Reg::V(i))) if i < 32 => Some(Type::V128),
            (
                GuestIsa::AArch64,
                Self::Arm64(Arm64Reg::Sp | Arm64Reg::TpidrEl0 | Arm64Reg::TpidrroEl0),
            ) => Some(Type::GuestAddress),
            (GuestIsa::AArch64, Self::Arm64(Arm64Reg::Nzcv | Arm64Reg::Fpcr | Arm64Reg::Fpsr)) => {
                Some(Type::I32)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntOp {
    Add,
    Sub,
    Mul,
    MulHighUnsigned,
    MulHighSigned,
    /// Return zero when the divisor is zero.
    DivUnsigned,
    /// Return zero for a zero divisor and the dividend for signed overflow.
    DivSigned,
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntPredicate {
    Eq,
    Ne,
    SignedLt,
    SignedLe,
    UnsignedLt,
    UnsignedLe,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShiftOp {
    Left,
    LogicalRight,
    ArithmeticRight,
    RotateLeft,
    RotateRight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShiftCount {
    /// Keep the lowest five or six bits (e.g. x86 scalar shifts).
    Mask5,
    Mask6,
    /// Reduce the unsigned count modulo the value's bit width.
    ModuloWidth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endian {
    Little,
    Big,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryAccess {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AddressConstraint {
    Canonical48,
}

/// The ordered access sequence is part of the IR. This form has no atomic operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryLayout {
    pub ty: Type,
    pub alignment: u8,
    pub endian: Endian,
}

impl MemoryLayout {
    pub const I64_PAIR_LE: Self = Self {
        ty: Type::I128,
        alignment: 1,
        endian: Endian::Little,
    };
    pub const V128_PAIR_LE: Self = Self {
        ty: Type::V128Pair,
        alignment: 1,
        endian: Endian::Little,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Op {
    RequireFeature(CpuFeature),
    /// Orders prior and subsequent guest data accesses in the serialized memory profile.
    Fence,
    Const {
        out: ValueId,
        ty: Type,
        bits: u128,
    },
    Int {
        out: ValueId,
        op: IntOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    Compare {
        out: ValueId,
        predicate: IntPredicate,
        lhs: ValueId,
        rhs: ValueId,
    },
    Shift {
        out: ValueId,
        op: ShiftOp,
        value: ValueId,
        count: ValueId,
        rule: ShiftCount,
    },
    ReverseBits {
        out: ValueId,
        value: ValueId,
    },
    CountLeadingBits {
        out: ValueId,
        value: ValueId,
        sign: bool,
    },
    Select {
        out: ValueId,
        condition: ValueId,
        yes: ValueId,
        no: ValueId,
    },
    IntToAddress {
        out: ValueId,
        value: ValueId,
    },
    AddressToInt {
        out: ValueId,
        value: ValueId,
    },
    /// Truncates or zero-extends an integer to the target integer type.
    IntCast {
        out: ValueId,
        value: ValueId,
        ty: Type,
    },
    /// Take the least significant bits of a V128 value as a scalar integer.
    VectorLow {
        out: ValueId,
        value: ValueId,
        ty: Type,
    },
    /// Put a scalar integer in the low bits of a zero-filled V128 value.
    VectorFromLow {
        out: ValueId,
        value: ValueId,
    },
    /// Count set bits independently in each byte of a V128 value.
    VectorCountOnes {
        out: ValueId,
        value: ValueId,
    },
    VectorExtract {
        out: ValueId,
        vector: ValueId,
        lane: u8,
        ty: Type,
    },
    VectorInsert {
        out: ValueId,
        vector: ValueId,
        lane: u8,
        value: ValueId,
    },
    /// Checks an instruction target before committing effects, optionally only on a taken edge.
    CheckTarget {
        address: ValueId,
        when: Option<ValueId>,
        constraint: AddressConstraint,
    },
    /// Check a base address before applying an instruction's memory offset.
    CheckMemoryAlignment {
        address: ValueId,
        alignment: u8,
        access: MemoryAccess,
    },
    ReadRegister {
        out: ValueId,
        register: Register,
    },
    WriteRegister {
        register: Register,
        value: ValueId,
    },
    /// Pairs a guest address with the runtime check performed by one access at this PC.
    Guard {
        out: ValueId,
        address: ValueId,
        access: MemoryAccess,
        layout: MemoryLayout,
    },
    Load {
        out: ValueId,
        guard: ValueId,
        layout: MemoryLayout,
    },
    Store {
        guard: ValueId,
        value: ValueId,
        layout: MemoryLayout,
    },
    CompareExchange {
        out: ValueId,
        guard: ValueId,
        expected: ValueId,
        replacement: ValueId,
        layout: MemoryLayout,
    },
    AtomicSwap {
        out: ValueId,
        guard: ValueId,
        replacement: ValueId,
        layout: MemoryLayout,
    },
    AtomicAdd {
        out: ValueId,
        guard: ValueId,
        addend: ValueId,
        layout: MemoryLayout,
    },
    AtomicBitUpdate {
        out: ValueId,
        guard: ValueId,
        mask: ValueId,
        set: bool,
        layout: MemoryLayout,
    },
    /// Read one contiguous 16-byte range into two little-endian scalar values.
    LoadPair64 {
        first: ValueId,
        second: ValueId,
        guard: ValueId,
    },
    /// Write two little-endian scalar values as one contiguous 16-byte range.
    StorePair64 {
        first: ValueId,
        second: ValueId,
        guard: ValueId,
    },
    LoadPair128 {
        first: ValueId,
        second: ValueId,
        guard: ValueId,
    },
    StorePair128 {
        first: ValueId,
        second: ValueId,
        guard: ValueId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Instruction {
    pub pc: GuestAddress,
    pub op: Op,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edge {
    pub target: BlockId,
    pub args: Vec<ValueId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Exit {
    Jump(Edge),
    Branch {
        condition: ValueId,
        yes: Edge,
        no: Edge,
    },
    /// Return to the dispatcher with a guest address.
    NextPc(ValueId),
    Trap {
        code: u32,
    },
    Fault(FaultClass),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrBlock {
    pub id: BlockId,
    pub span: CodeSpan,
    pub parameters: Vec<Parameter>,
    pub instructions: Vec<Instruction>,
    pub exit_pc: GuestAddress,
    pub exit: Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrFunction {
    /// The decoder/lifter declares required features out of band through the target profile.
    pub target: GuestTarget,
    pub entry: BlockId,
    pub blocks: Vec<IrBlock>,
}

impl IrFunction {
    pub fn verify(self) -> Result<VerifiedIrFunction, VerifyError> {
        verify::verify(self)
    }
}

impl fmt::Display for VerifiedIrFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let function = self.as_ref();
        writeln!(
            f,
            "bifrost-ir v{IR_VERSION} {:?} entry b{}",
            function.target.isa(),
            function.entry.0
        )?;
        for block in &function.blocks {
            write!(f, "b{}(", block.id.0)?;
            for (i, param) in block.parameters.iter().enumerate() {
                if i != 0 {
                    write!(f, ", ")?;
                }
                write!(f, "%{}: {:?}", param.value.0, param.ty)?;
            }
            writeln!(f, ") @ {:#x}+{}:", block.span.start.0, block.span.len)?;
            for inst in &block.instructions {
                writeln!(f, "  {:#x}  {:?}", inst.pc.0, inst.op)?;
            }
            writeln!(f, "  {:#x}  {:?}", block.exit_pc.0, block.exit)?;
        }
        Ok(())
    }
}
