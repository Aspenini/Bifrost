use bifrost_core::{CpuState, GuestAddress};
use bifrost_ir::{Arm64Reg, IntOp, IntPredicate, Register, ShiftCount, ShiftOp, Type, X86Reg};

#[derive(Clone, Copy)]
pub(crate) struct Value {
    pub(crate) bits: u128,
    ty: Type,
}

impl Value {
    pub(crate) fn insert_lane(self, lane: u8, value: Self) -> Self {
        let width = value.ty.bit_width();
        let shift = u32::from(lane) * width;
        let bits = (self.bits & !(mask(width) << shift)) | (value.bits << shift);
        Self::new(self.ty, bits)
    }
    pub(crate) fn count_leading_bits(self, sign: bool) -> Self {
        let width = self.ty.bit_width();
        let bits = if sign && self.signed() < 0 {
            self.bits ^ mask(width)
        } else {
            self.bits
        };
        let count = bits.leading_zeros() - (128 - width) - u32::from(sign);
        Self::new(self.ty, u128::from(count))
    }

    pub(crate) fn reverse_bits(self) -> Self {
        Self::new(
            self.ty,
            self.bits.reverse_bits() >> (128 - self.ty.bit_width()),
        )
    }

    pub(crate) const fn new(ty: Type, bits: u128) -> Self {
        Self {
            bits: bits & mask(ty.bit_width()),
            ty,
        }
    }

    pub(crate) fn integer(self, op: IntOp, rhs: Self) -> Self {
        let lhs = self.bits;
        let rhs = rhs.bits;
        let result = match op {
            IntOp::Add => lhs.wrapping_add(rhs),
            IntOp::Sub => lhs.wrapping_sub(rhs),
            IntOp::Mul => lhs.wrapping_mul(rhs),
            IntOp::MulHighUnsigned => (lhs * rhs) >> 64,
            IntOp::MulHighSigned => {
                let lhs = i128::from(lhs as i64);
                let rhs = i128::from(rhs as i64);
                ((lhs * rhs) >> 64) as u128
            }
            IntOp::DivUnsigned => lhs.checked_div(rhs).unwrap_or(0),
            IntOp::DivSigned => {
                let lhs = lhs as i64;
                let rhs = rhs as i64;
                if rhs == 0 {
                    0
                } else {
                    lhs.wrapping_div(rhs) as u128
                }
            }
            IntOp::And => lhs & rhs,
            IntOp::Or => lhs | rhs,
            IntOp::Xor => lhs ^ rhs,
        };
        Self::new(self.ty, result)
    }

    pub(crate) fn compare(self, predicate: IntPredicate, rhs: Self) -> Self {
        let matches = match predicate {
            IntPredicate::Eq => self.bits == rhs.bits,
            IntPredicate::Ne => self.bits != rhs.bits,
            IntPredicate::SignedLt => self.signed() < rhs.signed(),
            IntPredicate::SignedLe => self.signed() <= rhs.signed(),
            IntPredicate::UnsignedLt => self.bits < rhs.bits,
            IntPredicate::UnsignedLe => self.bits <= rhs.bits,
        };
        Self::new(Type::I1, u128::from(matches))
    }

    pub(crate) fn shift(self, op: ShiftOp, count: u128, rule: ShiftCount) -> Self {
        let width = self.ty.bit_width();
        let count = match rule {
            ShiftCount::Mask5 => (count & 31) as u32,
            ShiftCount::Mask6 => (count & 63) as u32,
            ShiftCount::ModuloWidth => (count % u128::from(width)) as u32,
        };
        let bits = match op {
            ShiftOp::Left if count < width => self.bits << count,
            ShiftOp::LogicalRight if count < width => self.bits >> count,
            ShiftOp::Left | ShiftOp::LogicalRight => 0,
            ShiftOp::ArithmeticRight => (self.signed() >> count.min(width - 1)) as u128,
            ShiftOp::RotateLeft => self.rotate_left(count % width),
            ShiftOp::RotateRight => self.rotate_left((width - count % width) % width),
        };
        Self::new(self.ty, bits)
    }

    fn signed(self) -> i128 {
        let shift = 128 - self.ty.bit_width();
        ((self.bits << shift) as i128) >> shift
    }

    fn rotate_left(self, count: u32) -> u128 {
        if count == 0 {
            self.bits
        } else {
            (self.bits << count) | (self.bits >> (self.ty.bit_width() - count))
        }
    }

    pub(crate) fn read(cpu: &CpuState, register: Register) -> Self {
        let (ty, bits) = match (cpu, register) {
            (CpuState::X86_64(state), Register::X86(reg)) => match reg {
                X86Reg::Gpr(index) => (Type::I64, u128::from(state.gpr[usize::from(index)])),
                X86Reg::Xmm(index) => (Type::V128, state.xmm[usize::from(index)]),
                X86Reg::Rflags => (Type::I64, u128::from(state.rflags)),
                X86Reg::FsBase => (Type::GuestAddress, u128::from(state.fs_base.0)),
                X86Reg::GsBase => (Type::GuestAddress, u128::from(state.gs_base.0)),
                X86Reg::Mxcsr => (Type::I32, u128::from(state.mxcsr)),
            },
            (CpuState::AArch64(state), Register::Arm64(reg)) => match reg {
                Arm64Reg::X(index) => (Type::I64, u128::from(state.x[usize::from(index)])),
                Arm64Reg::V(index) => (Type::V128, state.v[usize::from(index)]),
                Arm64Reg::Sp => (Type::GuestAddress, u128::from(state.sp.0)),
                Arm64Reg::Nzcv => (Type::I32, u128::from(state.nzcv)),
                Arm64Reg::TpidrEl0 => (Type::GuestAddress, u128::from(state.tpidr_el0.0)),
                Arm64Reg::TpidrroEl0 => (Type::GuestAddress, u128::from(state.tpidrro_el0.0)),
                Arm64Reg::Fpcr => (Type::I32, u128::from(state.fpcr)),
                Arm64Reg::Fpsr => (Type::I32, u128::from(state.fpsr)),
            },
            _ => unreachable!("verified register for the running ISA"),
        };
        Self::new(ty, bits)
    }

    pub(crate) fn write(self, cpu: &mut CpuState, register: Register) {
        let bits = self.bits;
        match (cpu, register) {
            (CpuState::X86_64(state), Register::X86(reg)) => match reg {
                X86Reg::Gpr(index) => state.gpr[usize::from(index)] = bits as u64,
                X86Reg::Xmm(index) => state.xmm[usize::from(index)] = bits,
                X86Reg::Rflags => state.rflags = bits as u64,
                X86Reg::FsBase => state.fs_base = GuestAddress(bits as u64),
                X86Reg::GsBase => state.gs_base = GuestAddress(bits as u64),
                X86Reg::Mxcsr => state.mxcsr = bits as u32,
            },
            (CpuState::AArch64(state), Register::Arm64(reg)) => match reg {
                Arm64Reg::X(index) => state.x[usize::from(index)] = bits as u64,
                Arm64Reg::V(index) => state.v[usize::from(index)] = bits,
                Arm64Reg::Sp => state.sp = GuestAddress(bits as u64),
                Arm64Reg::Nzcv => state.nzcv = bits as u32,
                Arm64Reg::TpidrEl0 => state.tpidr_el0 = GuestAddress(bits as u64),
                Arm64Reg::TpidrroEl0 => unreachable!("verified IR cannot write TPIDRRO_EL0"),
                Arm64Reg::Fpcr => state.fpcr = bits as u32,
                Arm64Reg::Fpsr => state.fpsr = bits as u32,
            },
            _ => unreachable!("verified register for the running ISA"),
        }
    }
}

const fn mask(width: u32) -> u128 {
    u128::MAX >> (128 - width)
}
