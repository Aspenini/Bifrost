//! Explicit, typed calls from guest ABI frames into registered native C functions.

use bifrost_abi::{AbiDescriptor, AbiError, Scalar, ScalarValue};
use bifrost_core::{CpuState, GuestMemory};
use std::fmt;

/// Each variant represents a compiler-checked C signature. No guest word is a host function pointer.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub enum NativeFunction {
    U64Binary(extern "C" fn(u64, u64) -> u64),
    I64Binary(extern "C" fn(i64, i64) -> i64),
    U64F64(extern "C" fn(u64, f64) -> f64),
}

impl NativeFunction {
    pub const fn scalar_signature(self) -> (&'static [Scalar], Scalar) {
        match self {
            Self::U64Binary(_) => (&[Scalar::U64, Scalar::U64], Scalar::U64),
            Self::I64Binary(_) => (&[Scalar::I64, Scalar::I64], Scalar::I64),
            Self::U64F64(_) => (&[Scalar::U64, Scalar::F64], Scalar::F64),
        }
    }

    /// Reads the guest call frame, invokes the typed native function and simulates its return.
    pub fn invoke(
        self,
        abi: &AbiDescriptor,
        cpu: &mut CpuState,
        memory: &mut dyn GuestMemory,
    ) -> Result<(), BridgeError> {
        AbiDescriptor::native().ok_or(BridgeError::UnsupportedHost)?;
        let return_site = abi.capture_return(cpu, memory)?;
        let (arguments, result) = self.scalar_signature();
        let locations = abi.argument_locations(arguments)?;
        let [left, right] = locations.as_slice() else {
            unreachable!("fixed binary signatures");
        };
        let left = abi.read_word(cpu, memory, *left)?;
        let right = abi.read_word(cpu, memory, *right)?;
        let output = match self {
            Self::U64Binary(function) => ScalarValue::U64(function(left, right)),
            Self::I64Binary(function) => ScalarValue::I64(function(left as i64, right as i64)),
            Self::U64F64(function) => ScalarValue::F64(function(left, f64::from_bits(right))),
        };
        abi.write_word(cpu, memory, abi.scalar_return(result), output.bits())?;
        return_site.apply(cpu);
        Ok(())
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum BridgeError {
    Abi(AbiError),
    UnsupportedHost,
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Abi(error) => error.fmt(f),
            Self::UnsupportedHost => f.write_str("the host native C ABI is unavailable"),
        }
    }
}

impl std::error::Error for BridgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Abi(error) => Some(error),
            _ => None,
        }
    }
}

impl From<AbiError> for BridgeError {
    fn from(error: AbiError) -> Self {
        Self::Abi(error)
    }
}
