use bifrost_core::{Arm64State, GuestAddress};
use bifrost_decoder_arm64::{
    FloatBinary, FloatBinaryKind, FloatCompare, FloatConvert, FloatMultiplyAdd, FloatUnary,
    FloatUnaryKind, IntegerToFloat,
};

use super::ExecutionError;

const INVALID: u32 = 1;
const DIVIDE_BY_ZERO: u32 = 1 << 1;
const OVERFLOW: u32 = 1 << 2;
const UNDERFLOW: u32 = 1 << 3;
const INEXACT: u32 = 1 << 4;
const INPUT_DENORMAL: u32 = 1 << 7;
const FLUSH_TO_ZERO: u32 = 1 << 24;
const DEFAULT_NAN: u32 = 1 << 25;
const ROUNDING_MASK: u32 = 3 << 22;

pub(super) fn multiply_add(
    cpu: &mut Arm64State,
    pc: GuestAddress,
    op: FloatMultiplyAdd,
) -> Result<(), ExecutionError> {
    check_control(cpu, pc)?;
    let format = if op.double {
        Format::DOUBLE
    } else {
        Format::SINGLE
    };
    let mut input_flags = 0;
    let mut read = |reg, negate| {
        let bits = cpu.v[usize::from(reg)] as u64 ^ format.sign_bit(negate);
        format.unpack_input(bits, cpu.fpcr, &mut input_flags)
    };
    let lhs = read(op.lhs, op.negate_product);
    let addend = read(op.addend, op.negate_addend);
    let (bits, flags) = format.fma(lhs, read(op.rhs, false), addend, cpu.fpcr);
    write_result(cpu, pc, op.dst, bits, flags | input_flags);
    Ok(())
}

pub(super) fn integer_to_float(
    cpu: &mut Arm64State,
    pc: GuestAddress,
    op: IntegerToFloat,
) -> Result<(), ExecutionError> {
    check_control(cpu, pc)?;
    let register = cpu.x.get(usize::from(op.src)).copied().unwrap_or(0);
    let (sign, magnitude) = match (op.signed, op.word) {
        (true, true) => {
            let value = register as i32;
            (value < 0, u64::from(value.unsigned_abs()))
        }
        (true, false) => {
            let value = register as i64;
            (value < 0, value.unsigned_abs())
        }
        (false, true) => (false, u64::from(register as u32)),
        (false, false) => (false, register),
    };
    let format = if op.double {
        Format::DOUBLE
    } else {
        Format::SINGLE
    };
    let (bits, flags) = if magnitude == 0 {
        (0, 0)
    } else {
        let top = (63 - magnitude.leading_zeros()) as usize;
        format.round(
            Wide::shifted(u128::from(magnitude), 0),
            top,
            0,
            sign,
            cpu.fpcr,
        )
    };
    write_result(cpu, pc, op.dst, bits, flags);
    Ok(())
}

fn check_control(cpu: &Arm64State, pc: GuestAddress) -> Result<(), ExecutionError> {
    if cpu.fpcr & !(DEFAULT_NAN | ROUNDING_MASK | FLUSH_TO_ZERO) != 0 {
        return Err(ExecutionError::UnsupportedArm64Fpcr { pc, fpcr: cpu.fpcr });
    }
    Ok(())
}

pub(super) fn convert(
    cpu: &mut Arm64State,
    pc: GuestAddress,
    op: FloatConvert,
) -> Result<(), ExecutionError> {
    check_control(cpu, pc)?;
    let (from, to) = if op.to_double {
        (Format::SINGLE, Format::DOUBLE)
    } else {
        (Format::DOUBLE, Format::SINGLE)
    };
    let mut input_flags = 0;
    let value = from.unpack_input(
        cpu.v[usize::from(op.src)] as u64,
        cpu.fpcr,
        &mut input_flags,
    );
    let (bits, flags) = if from.is_nan(value) {
        let source_quiet = 1 << (from.fraction - 1);
        let dest_quiet = 1 << (to.fraction - 1);
        let flags = if value.fraction & source_quiet == 0 {
            INVALID
        } else {
            0
        };
        let bits = if cpu.fpcr & DEFAULT_NAN != 0 {
            to.infinity(false) | dest_quiet
        } else {
            let payload = if op.to_double {
                value.fraction << (to.fraction - from.fraction)
            } else {
                value.fraction >> (from.fraction - to.fraction)
            };
            to.infinity(value.sign) | dest_quiet | payload
        };
        (bits, flags)
    } else if value.exponent == from.infinity_exponent() {
        (to.infinity(value.sign), 0)
    } else {
        let (magnitude, base) = from.finite(value);
        if magnitude == 0 {
            (to.sign_bit(value.sign), 0)
        } else {
            let top = (63 - magnitude.leading_zeros()) as usize;
            to.round(
                Wide::shifted(magnitude.into(), 0),
                top,
                base,
                value.sign,
                cpu.fpcr,
            )
        }
    };
    write_result(cpu, pc, op.dst, bits, flags | input_flags);
    Ok(())
}

pub(super) fn compare(
    cpu: &mut Arm64State,
    pc: GuestAddress,
    op: FloatCompare,
) -> Result<(), ExecutionError> {
    use std::cmp::Ordering;

    check_control(cpu, pc)?;
    if let Some(when) = op.when
        && !condition_holds(cpu.nzcv, when.code)
    {
        cpu.nzcv = u32::from(when.fallback_nzcv) << 28;
        cpu.pc = GuestAddress(pc.0.wrapping_add(4));
        return Ok(());
    }
    let format = if op.double {
        Format::DOUBLE
    } else {
        Format::SINGLE
    };
    let lhs = format.unpack_input(cpu.v[usize::from(op.lhs)] as u64, cpu.fpcr, &mut cpu.fpsr);
    let rhs = format.unpack_input(
        op.rhs.map_or(0, |index| cpu.v[usize::from(index)] as u64),
        cpu.fpcr,
        &mut cpu.fpsr,
    );
    let sign = format.sign_bit(true);
    let order = if format.is_nan(lhs) || format.is_nan(rhs) {
        let signaling =
            |value| format.is_nan(value) && value.fraction & (1 << (format.fraction - 1)) == 0;
        if op.signal_quiet_nan || signaling(lhs) || signaling(rhs) {
            cpu.fpsr |= INVALID;
        }
        None
    } else if (lhs.bits | rhs.bits) & !sign == 0 {
        Some(Ordering::Equal)
    } else {
        let key = |value: Operand| {
            if value.sign {
                !value.bits
            } else {
                value.bits | sign
            }
        };
        // Positive and negative use separate ranges because binary32 uses a 32-bit sign.
        let order = match (lhs.sign, rhs.sign) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => key(lhs).cmp(&key(rhs)),
        };
        Some(order)
    };
    cpu.nzcv = match order {
        Some(Ordering::Less) => 0b1000,
        Some(Ordering::Equal) => 0b0110,
        Some(Ordering::Greater) => 0b0010,
        None => 0b0011,
    } << 28;
    cpu.pc = GuestAddress(pc.0.wrapping_add(4));
    Ok(())
}

fn condition_holds(nzcv: u32, condition: u8) -> bool {
    let n = nzcv & (1 << 31) != 0;
    let z = nzcv & (1 << 30) != 0;
    let c = nzcv & (1 << 29) != 0;
    let v = nzcv & (1 << 28) != 0;
    let base = match condition >> 1 {
        0 => z,
        1 => c,
        2 => n,
        3 => v,
        4 => c && !z,
        5 => n == v,
        6 => n == v && !z,
        _ => return true,
    };
    base ^ (condition & 1 != 0)
}

fn write_result(cpu: &mut Arm64State, pc: GuestAddress, dst: u8, bits: u64, flags: u32) {
    cpu.v[usize::from(dst)] = u128::from(bits);
    cpu.fpsr |= flags;
    cpu.pc = GuestAddress(pc.0.wrapping_add(4));
}

pub(super) fn unary(
    cpu: &mut Arm64State,
    pc: GuestAddress,
    op: FloatUnary,
) -> Result<(), ExecutionError> {
    check_control(cpu, pc)?;
    if op.kind == FloatUnaryKind::SquareRoot {
        let format = if op.double {
            Format::DOUBLE
        } else {
            Format::SINGLE
        };
        let mut input_flags = 0;
        let value = format.unpack_input(
            cpu.v[usize::from(op.src)] as u64,
            cpu.fpcr,
            &mut input_flags,
        );
        let (bits, flags) = format.square_root(value, cpu.fpcr);
        write_result(cpu, pc, op.dst, bits, flags | input_flags);
        return Ok(());
    }
    let lane_width = if op.double { 64 } else { 32 };
    let sign_mask = (1..=op.lanes).fold(0u128, |mask, lane| {
        mask | (1 << (u32::from(lane) * lane_width - 1))
    });
    let width = u32::from(op.lanes) * lane_width;
    let bits = cpu.v[usize::from(op.src)] & (u128::MAX >> (128 - width));
    cpu.v[usize::from(op.dst)] = if op.kind == FloatUnaryKind::Negate {
        bits ^ sign_mask
    } else {
        bits & !sign_mask
    };
    cpu.pc = GuestAddress(pc.0.wrapping_add(4));
    Ok(())
}

pub(super) fn binary(
    cpu: &mut Arm64State,
    pc: GuestAddress,
    op: FloatBinary,
) -> Result<(), ExecutionError> {
    check_control(cpu, pc)?;
    let format = if op.double {
        Format::DOUBLE
    } else {
        Format::SINGLE
    };
    let mut input_flags = 0;
    let lhs = format.unpack_input(
        cpu.v[usize::from(op.lhs)] as u64,
        cpu.fpcr,
        &mut input_flags,
    );
    let rhs = format.unpack_input(
        cpu.v[usize::from(op.rhs)] as u64,
        cpu.fpcr,
        &mut input_flags,
    );
    let (bits, flags) = match op.kind {
        FloatBinaryKind::Divide => format.divide(lhs, rhs, cpu.fpcr),
        FloatBinaryKind::Multiply => {
            let zero = format.unpack(format.sign_bit(lhs.sign ^ rhs.sign));
            format.fma(lhs, rhs, zero, cpu.fpcr)
        }
        FloatBinaryKind::Add | FloatBinaryKind::Subtract => {
            let one = format.unpack((format.bias as u64) << format.fraction);
            let rhs = if op.kind == FloatBinaryKind::Subtract && !format.is_nan(rhs) {
                format.unpack(rhs.bits ^ format.sign_bit(true))
            } else {
                rhs
            };
            // FMA examines the addend first when selecting a NaN payload.
            format.fma(rhs, one, lhs, cpu.fpcr)
        }
    };
    write_result(cpu, pc, op.dst, bits, flags | input_flags);
    Ok(())
}

#[derive(Clone, Copy)]
struct Format {
    fraction: u32,
    exponent: u32,
    bias: i32,
}

#[derive(Clone, Copy)]
struct Operand {
    sign: bool,
    fraction: u64,
    exponent: u64,
    bits: u64,
}

impl Format {
    const SINGLE: Self = Self {
        fraction: 23,
        exponent: 8,
        bias: 127,
    };
    const DOUBLE: Self = Self {
        fraction: 52,
        exponent: 11,
        bias: 1023,
    };

    fn unpack(self, bits: u64) -> Operand {
        Operand {
            sign: bits & self.sign_bit(true) != 0,
            fraction: bits & ((1 << self.fraction) - 1),
            exponent: (bits >> self.fraction) & self.infinity_exponent(),
            bits: bits & (self.sign_bit(true) | (self.sign_bit(true) - 1)),
        }
    }

    fn unpack_input(self, bits: u64, fpcr: u32, flags: &mut u32) -> Operand {
        let value = self.unpack(bits);
        if fpcr & FLUSH_TO_ZERO != 0 && value.exponent == 0 && value.fraction != 0 {
            *flags |= INPUT_DENORMAL;
            self.unpack(self.sign_bit(value.sign))
        } else {
            value
        }
    }

    fn sign_bit(self, sign: bool) -> u64 {
        u64::from(sign) << (self.fraction + self.exponent)
    }

    fn infinity_exponent(self) -> u64 {
        (1 << self.exponent) - 1
    }

    fn infinity(self, sign: bool) -> u64 {
        self.sign_bit(sign) | (self.infinity_exponent() << self.fraction)
    }

    fn is_nan(self, op: Operand) -> bool {
        op.exponent == self.infinity_exponent() && op.fraction != 0
    }

    fn square_root(self, value: Operand, fpcr: u32) -> (u64, u32) {
        let quiet = 1 << (self.fraction - 1);
        let default_nan = self.infinity(false) | quiet;
        if self.is_nan(value) {
            let flags = if value.fraction & quiet == 0 {
                INVALID
            } else {
                0
            };
            let bits = if fpcr & DEFAULT_NAN != 0 {
                default_nan
            } else {
                value.bits | quiet
            };
            return (bits, flags);
        }
        let (fraction, exponent) = self.finite(value);
        if fraction == 0 {
            return (value.bits, 0);
        }
        if value.sign {
            return (default_nan, INVALID);
        }
        if value.exponent == self.infinity_exponent() {
            return (value.bits, 0);
        }

        // Scale the exact radicand so the root has extra low bits for rounding.
        // Keep the power of two even so it can be applied after the square root.
        let width = 64 - fraction.leading_zeros();
        let shift = 2 * (self.fraction + 4) - width;
        let shift = shift + ((exponent - shift as i32) & 1) as u32;
        let scaled = u128::from(fraction) << shift;
        let root = scaled.isqrt();
        let rounded_input = root | u128::from(root * root != scaled);
        let top = (127 - rounded_input.leading_zeros()) as usize;
        self.round(
            Wide::shifted(rounded_input, 0),
            top,
            (exponent - shift as i32) / 2,
            false,
            fpcr,
        )
    }

    fn divide(self, lhs: Operand, rhs: Operand, fpcr: u32) -> (u64, u32) {
        if self.is_nan(lhs) || self.is_nan(rhs) {
            // Use the same signaling-first NaN selection as the other binary operations.
            let one = self.unpack((self.bias as u64) << self.fraction);
            return self.fma(rhs, one, lhs, fpcr);
        }
        let sign = lhs.sign ^ rhs.sign;
        let lhs_infinite = lhs.exponent == self.infinity_exponent();
        let rhs_infinite = rhs.exponent == self.infinity_exponent();
        let (numerator, numerator_exp) = self.finite(lhs);
        let (denominator, denominator_exp) = self.finite(rhs);
        if (lhs_infinite && rhs_infinite) || (numerator == 0 && denominator == 0) {
            return (self.infinity(false) | (1 << (self.fraction - 1)), INVALID);
        }
        if lhs_infinite {
            return (self.infinity(sign), 0);
        }
        if denominator == 0 {
            return (self.infinity(sign), DIVIDE_BY_ZERO);
        }
        if rhs_infinite || numerator == 0 {
            return (self.sign_bit(sign), 0);
        }

        // Keep at least two low bits beyond the destination precision, with
        // any remaining nonzero quotient tail folded into the last bit.
        let shift = self.fraction + 3 + numerator.leading_zeros() - denominator.leading_zeros();
        let scaled = u128::from(numerator) << shift;
        let denominator = u128::from(denominator);
        let quotient = (scaled / denominator) | u128::from(!scaled.is_multiple_of(denominator));
        let base = numerator_exp - denominator_exp - shift as i32;
        let top = (127 - quotient.leading_zeros()) as usize;
        self.round(Wide::shifted(quotient, 0), top, base, sign, fpcr)
    }

    fn finite(self, operand: Operand) -> (u64, i32) {
        let fraction = operand.fraction | (u64::from(operand.exponent != 0) << self.fraction);
        let exponent = operand.exponent.max(1) as i32 - self.bias - self.fraction as i32;
        (fraction, exponent)
    }

    fn fma(self, lhs: Operand, rhs: Operand, addend: Operand, fpcr: u32) -> (u64, u32) {
        let quiet_bit = 1 << (self.fraction - 1);
        let default_nan = self.infinity(false) | quiet_bit;
        let is_nan = |op| self.is_nan(op);
        let is_inf = |op: Operand| op.exponent == self.infinity_exponent() && op.fraction == 0;
        let is_zero = |op: Operand| op.exponent == 0 && op.fraction == 0;
        let mul_invalid = (is_inf(lhs) && is_zero(rhs)) || (is_inf(rhs) && is_zero(lhs));
        let operands = [addend, lhs, rhs];
        let signaling = operands
            .into_iter()
            .find(|&op| is_nan(op) && op.fraction & quiet_bit == 0);
        if let Some(op) = signaling {
            return (
                if fpcr & DEFAULT_NAN != 0 {
                    default_nan
                } else {
                    op.bits | quiet_bit
                },
                INVALID,
            );
        }
        if mul_invalid && is_nan(addend) {
            return (default_nan, INVALID);
        }
        if let Some(op) = operands.into_iter().find(|&op| is_nan(op)) {
            return (
                if fpcr & DEFAULT_NAN != 0 {
                    default_nan
                } else {
                    op.bits
                },
                0,
            );
        }
        let product_sign = lhs.sign ^ rhs.sign;
        if mul_invalid
            || ((is_inf(lhs) || is_inf(rhs)) && is_inf(addend) && product_sign != addend.sign)
        {
            return (default_nan, INVALID);
        }
        if is_inf(lhs) || is_inf(rhs) {
            return (self.infinity(product_sign), 0);
        }
        if is_inf(addend) {
            return (addend.bits, 0);
        }

        let (lhs, lhs_exp) = self.finite(lhs);
        let (rhs, rhs_exp) = self.finite(rhs);
        let (augend, addend_exp) = self.finite(addend);
        let product_exp = lhs_exp + rhs_exp;
        let base = product_exp.min(addend_exp);
        let product = Wide::shifted(
            u128::from(lhs) * u128::from(rhs),
            (product_exp - base) as usize,
        );
        let augend = Wide::shifted(u128::from(augend), (addend_exp - base) as usize);
        let (magnitude, sign) = if product_sign == addend.sign {
            (product.add(augend), product_sign)
        } else if product.0.iter().rev().cmp(augend.0.iter().rev()).is_ge() {
            (product.sub(augend), product_sign)
        } else {
            (augend.sub(product), addend.sign)
        };
        let rounding = (fpcr >> 22) & 3;
        let Some(top) = magnitude.top() else {
            let negative = if product_sign == addend.sign {
                product_sign
            } else {
                rounding == 2
            };
            return (self.sign_bit(negative), 0);
        };
        self.round(magnitude, top, base, sign, fpcr)
    }

    fn round(self, magnitude: Wide, top: usize, base: i32, sign: bool, fpcr: u32) -> (u64, u32) {
        let minimum = 1 - self.bias;
        let mut exponent = base + top as i32;
        let tiny = exponent < minimum;
        // In the base FP profile, flush tiny outputs before rounding and set UFC.
        if tiny && fpcr & FLUSH_TO_ZERO != 0 {
            return (self.sign_bit(sign), UNDERFLOW);
        }
        let rounding = (fpcr >> 22) & 3;
        let shift = exponent.max(minimum) - self.fraction as i32 - base;
        let (mut significand, midpoint, sticky) = if shift > 0 {
            let shift = shift as usize;
            (
                magnitude.low_after_shift(shift),
                magnitude.bit(shift - 1),
                magnitude.any_below(shift - 1),
            )
        } else {
            (magnitude.0[0] << (-shift as u32), false, false)
        };
        let inexact = midpoint || sticky;
        let round_up = match rounding {
            0 => midpoint && (sticky || significand & 1 != 0),
            1 => inexact && !sign,
            2 => inexact && sign,
            _ => false,
        };
        significand += u64::from(round_up);
        let normal_bit = 1 << self.fraction;
        if significand == normal_bit << 1 {
            significand >>= 1;
            exponent += 1;
        }
        if exponent > self.bias {
            let infinity = self.infinity(sign);
            let use_infinity = rounding == 0 || (rounding == 1 && !sign) || (rounding == 2 && sign);
            return (
                if use_infinity { infinity } else { infinity - 1 },
                OVERFLOW | INEXACT,
            );
        }
        let biased = if significand < normal_bit {
            0
        } else {
            (exponent.max(minimum) + self.bias) as u64
        };
        let bits =
            self.sign_bit(sign) | (biased << self.fraction) | (significand & (normal_bit - 1));
        let flags = if inexact {
            INEXACT | if tiny { UNDERFLOW } else { 0 }
        } else {
            0
        };
        (bits, flags)
    }
}

// Binary64 exponents can differ by over 3,000 bits between product and addend.
// Keep the exact integer magnitude until the single architectural rounding step.
const LIMBS: usize = 52;
struct Wide([u64; LIMBS]);

impl Wide {
    fn shifted(value: u128, shift: usize) -> Self {
        let mut result = Self([0; LIMBS]);
        let limb = shift / 64;
        let bits = shift % 64;
        let low = value as u64;
        let high = (value >> 64) as u64;
        result.0[limb] = low << bits;
        result.0[limb + 1] = high << bits;
        if bits != 0 {
            result.0[limb + 1] |= low >> (64 - bits);
            result.0[limb + 2] = high >> (64 - bits);
        }
        result
    }

    fn add(mut self, rhs: Self) -> Self {
        let mut carry = false;
        for (left, right) in self.0.iter_mut().zip(rhs.0) {
            (*left, carry) = left.carrying_add(right, carry);
        }
        debug_assert!(!carry);
        self
    }

    fn sub(mut self, rhs: Self) -> Self {
        let mut borrow = false;
        for (left, right) in self.0.iter_mut().zip(rhs.0) {
            (*left, borrow) = left.borrowing_sub(right, borrow);
        }
        debug_assert!(!borrow);
        self
    }

    fn top(&self) -> Option<usize> {
        let limb = self.0.iter().rposition(|&word| word != 0)?;
        Some(limb * 64 + 63 - self.0[limb].leading_zeros() as usize)
    }

    fn low_after_shift(&self, shift: usize) -> u64 {
        let limb = shift / 64;
        let bits = shift % 64;
        let low = self.0.get(limb).copied().unwrap_or(0) >> bits;
        if bits == 0 {
            low
        } else {
            low | (self.0.get(limb + 1).copied().unwrap_or(0) << (64 - bits))
        }
    }

    fn bit(&self, bit: usize) -> bool {
        self.0
            .get(bit / 64)
            .is_some_and(|word| word & (1 << (bit % 64)) != 0)
    }

    fn any_below(&self, bit: usize) -> bool {
        let limb = bit / 64;
        self.0[..limb.min(LIMBS)].iter().any(|&word| word != 0)
            || self
                .0
                .get(limb)
                .is_some_and(|word| word & ((1 << (bit % 64)) - 1) != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_signed_zero_numbers_and_nan_with_architectural_flags() {
        let pc = GuestAddress(0x1000);
        let mut cpu = Arm64State::default();
        let op = FloatCompare {
            lhs: 0,
            rhs: Some(1),
            double: false,
            signal_quiet_nan: false,
            when: None,
        };
        cpu.v[0] = u128::from((-0.0f32).to_bits());
        compare(&mut cpu, pc, op).unwrap();
        assert_eq!(cpu.nzcv, 0x6000_0000);
        cpu.v[0] = u128::from((-2.0f32).to_bits());
        cpu.v[1] = u128::from((-1.0f32).to_bits());
        compare(&mut cpu, pc, op).unwrap();
        assert_eq!(cpu.nzcv, 0x8000_0000);
        cpu.v[0] = u128::from(f32::NAN.to_bits());
        compare(&mut cpu, pc, op).unwrap();
        assert_eq!(cpu.nzcv, 0x3000_0000);
        assert_eq!(cpu.fpsr, 0);
        compare(
            &mut cpu,
            pc,
            FloatCompare {
                signal_quiet_nan: true,
                ..op
            },
        )
        .unwrap();
        assert_eq!(cpu.nzcv, 0x3000_0000);
        assert_eq!(cpu.fpsr, INVALID);
    }

    #[test]
    fn rejected_fp_controls_leave_guest_state_unchanged() {
        let op = FloatMultiplyAdd {
            dst: 0,
            lhs: 0,
            rhs: 0,
            addend: 0,
            double: true,
            negate_addend: false,
            negate_product: false,
        };
        let pc = GuestAddress(0x1000);
        let mut cpu = Arm64State {
            pc,
            fpcr: 1 << 8,
            ..Default::default()
        };
        cpu.v[0] = u128::MAX;
        let before = cpu.clone();
        assert!(matches!(
            multiply_add(&mut cpu, pc, op),
            Err(ExecutionError::UnsupportedArm64Fpcr {
                fpcr: 0x0000_0100,
                ..
            })
        ));
        assert_eq!(cpu, before);
    }

    #[test]
    fn flush_to_zero_preserves_sign_and_distinguishes_input_and_output_flags() {
        let pc = GuestAddress(0x1000);
        let op = FloatBinary {
            dst: 0,
            lhs: 1,
            rhs: 2,
            double: true,
            kind: FloatBinaryKind::Multiply,
        };
        let mut cpu = Arm64State {
            pc,
            fpcr: FLUSH_TO_ZERO | (2 << 22),
            ..Default::default()
        };
        cpu.v[1] = u128::from((-f64::MIN_POSITIVE).to_bits());
        cpu.v[2] = u128::from(0.5f64.to_bits());
        binary(&mut cpu, pc, op).unwrap();
        assert_eq!(cpu.v[0], 1 << 63);
        assert_eq!(cpu.fpsr, UNDERFLOW);

        cpu.v[1] = (1 << 63) | 1;
        binary(&mut cpu, pc, op).unwrap();
        assert_eq!(cpu.v[0], 1 << 63);
        assert_eq!(cpu.fpsr, UNDERFLOW | INPUT_DENORMAL);

        cpu.fpsr = 0;
        cpu.v[1] = (1 << 31) | 1;
        convert(
            &mut cpu,
            pc,
            FloatConvert {
                dst: 0,
                src: 1,
                to_double: true,
            },
        )
        .unwrap();
        assert_eq!(cpu.v[0], 1 << 63);
        assert_eq!(cpu.fpsr, INPUT_DENORMAL);
    }

    #[test]
    fn nearest_finite_results_match_host_fused_operations() {
        let mut seed = 0x238b_46e4_f4f6_030d_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..5000 {
            let [a, b, c] = std::array::from_fn(|_| f64::from_bits(next()));
            if a.is_finite() && b.is_finite() && c.is_finite() {
                let format = Format::DOUBLE;
                let [lhs, rhs, addend] = [a, b, c].map(|value| format.unpack(value.to_bits()));
                let (actual, _) = format.fma(lhs, rhs, addend, 0);
                assert_eq!(actual, a.mul_add(b, c).to_bits(), "{a:e} * {b:e} + {c:e}");
                let (actual, _) = format.divide(lhs, rhs, 0);
                assert_eq!(actual, (a / b).to_bits(), "{a:e} / {b:e}");
                let (actual, _) = format.square_root(format.unpack(a.abs().to_bits()), 0);
                assert_eq!(actual, a.abs().sqrt().to_bits(), "sqrt({a:e})");
            }
            let [a, b, c] = std::array::from_fn(|_| f32::from_bits(next() as u32));
            if a.is_finite() && b.is_finite() && c.is_finite() {
                let format = Format::SINGLE;
                let [lhs, rhs, addend] =
                    [a, b, c].map(|value| format.unpack(value.to_bits().into()));
                let (actual, _) = format.fma(lhs, rhs, addend, 0);
                assert_eq!(
                    actual,
                    u64::from(a.mul_add(b, c).to_bits()),
                    "{a:e} * {b:e} + {c:e}"
                );
                let (actual, _) = format.divide(lhs, rhs, 0);
                assert_eq!(actual, u64::from((a / b).to_bits()), "{a:e} / {b:e}");
                let (actual, _) = format.square_root(format.unpack(a.abs().to_bits().into()), 0);
                assert_eq!(actual, u64::from(a.abs().sqrt().to_bits()), "sqrt({a:e})");
            }
        }
    }

    #[test]
    fn honors_rounding_and_sets_cumulative_status_for_exceptional_results() {
        let fmt = Format::DOUBLE;
        let compute = |a: u64, b: u64, c: u64, fpcr| {
            fmt.fma(fmt.unpack(a), fmt.unpack(b), fmt.unpack(c), fpcr)
        };
        let one = 1.0f64.to_bits();
        let half_ulp = (2.0f64.powi(-53)).to_bits();
        let infinity = f64::INFINITY.to_bits();
        let max = f64::MAX.to_bits();
        assert_eq!(compute(one, one, half_ulp, 0), (one, INEXACT));
        assert_eq!(compute(one, one, half_ulp, 1 << 22), (one + 1, INEXACT));
        assert_eq!(compute(one, 1 << 63 | one, one, 2 << 22), (1 << 63, 0));
        assert_eq!(compute(1, 0.5f64.to_bits(), 0, 0), (0, UNDERFLOW | INEXACT));
        assert_eq!(compute(max, max, 0, 3 << 22), (max, OVERFLOW | INEXACT));
        assert_eq!(compute(infinity, 0, one, 0), (f64::NAN.to_bits(), INVALID));
        let payload = infinity | 248;
        assert_eq!(
            compute(payload, one, one, 0),
            (payload | (1 << 51), INVALID)
        );
        assert_eq!(
            compute(payload, one, one, DEFAULT_NAN),
            (f64::NAN.to_bits(), INVALID)
        );
    }
}
