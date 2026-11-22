//! Explicit, typed calls from guest ABI frames into registered native C functions.

#[allow(unsafe_code)]
mod buffer;
mod registry;

pub use buffer::{BufferMode, MAX_BUFFER_BYTES, NativeBuffer};
pub use registry::{BridgeRegistry, HandleId, NativeToken};

use bifrost_abi::{
    AbiDescriptor, AbiError, Argument, ArgumentLocation, CallReturn, Location, Pair64,
    ResultLocation, Scalar,
};
use bifrost_core::{CpuState, GuestAddress, GuestMemory, MemoryError};
use std::fmt;

const WORD: Argument = Argument::Scalar(Scalar::U64);

/// Each variant represents a compiler-checked C signature.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub enum NativeFunction {
    U64Binary(extern "C" fn(u64, u64) -> u64),
    I64Binary(extern "C" fn(i64, i64) -> i64),
    U64F64(extern "C" fn(u64, f64) -> f64),
    Pair(extern "C" fn(Pair64) -> Pair64),
    Buffer(NativeBuffer),
    HandleU64(extern "C" fn(u64, u64) -> u64),
}

impl NativeFunction {
    pub const fn signature(self) -> (&'static [Argument], Argument) {
        match self {
            Self::U64Binary(_) | Self::Buffer(_) | Self::HandleU64(_) => (&[WORD, WORD], WORD),
            Self::I64Binary(_) => (
                &[Argument::Scalar(Scalar::I64); 2],
                Argument::Scalar(Scalar::I64),
            ),
            Self::U64F64(_) => (
                &[WORD, Argument::Scalar(Scalar::F64)],
                Argument::Scalar(Scalar::F64),
            ),
            Self::Pair(_) => (&[Argument::Pair64], Argument::Pair64),
        }
    }

    /// Invokes an explicitly selected function. Module-owned handles require a registry.
    pub fn invoke(
        self,
        abi: &AbiDescriptor,
        cpu: &mut CpuState,
        memory: &mut dyn GuestMemory,
    ) -> Result<(), BridgeError> {
        let mut call = PreparedCall::prepare(self, abi, cpu, memory)?;
        let result = call.call_native()?;
        call.finish(cpu, memory, result)
    }
}

pub(crate) struct PreparedCall<'a> {
    abi: &'a AbiDescriptor,
    function: NativeFunction,
    input: Input,
    output: Output,
    resume: CallReturn,
}

enum Input {
    Words(u64, u64),
    Pair(Pair64),
    Buffer {
        bytes: Vec<u8>,
        write_back: Option<GuestAddress>,
    },
    Handle {
        id: u64,
        value: u64,
        native: Option<u64>,
    },
}

pub(crate) enum NativeResult {
    Word(u64),
    Pair(Pair64),
}

enum Output {
    Word(Location),
    Pair([Location; 2]),
    Indirect {
        address: GuestAddress,
        echo: Location,
    },
}

impl<'a> PreparedCall<'a> {
    fn prepare(
        function: NativeFunction,
        abi: &'a AbiDescriptor,
        cpu: &CpuState,
        memory: &dyn GuestMemory,
    ) -> Result<Self, BridgeError> {
        AbiDescriptor::native().ok_or(BridgeError::UnsupportedHost)?;
        let resume = abi.capture_return(cpu, memory)?;
        let (arguments, result) = function.signature();
        let layout = abi.layout(arguments, result)?;
        let output = match layout.result {
            ResultLocation::Word(location) => Output::Word(location),
            ResultLocation::Pair(locations) => Output::Pair(locations),
            ResultLocation::IndirectPair { pointer, echo } => {
                let address = GuestAddress(abi.read_word(cpu, memory, pointer)?);
                check_pair(abi, address, 8)?;
                memory.check_write(address, 16)?;
                Output::Indirect { address, echo }
            }
        };
        let input = match layout.arguments.as_slice() {
            [ArgumentLocation::Pair([first, second])] => Input::Pair(Pair64 {
                first: abi.read_word(cpu, memory, *first)?,
                second: abi.read_word(cpu, memory, *second)?,
            }),
            [ArgumentLocation::IndirectPair(location)] => {
                let address = GuestAddress(abi.read_word(cpu, memory, *location)?);
                check_pair(abi, address, 16)?;
                let mut bytes = [0; 16];
                memory.read(address, &mut bytes)?;
                Input::Pair(Pair64::from_guest_bytes(bytes))
            }
            [
                ArgumentLocation::Word(first),
                ArgumentLocation::Word(second),
            ] => {
                let first = abi.read_word(cpu, memory, *first)?;
                let second = abi.read_word(cpu, memory, *second)?;
                match function {
                    NativeFunction::Buffer(binding) => {
                        let address = GuestAddress(first);
                        let len = usize::try_from(second)
                            .ok()
                            .filter(|&len| len <= binding.limit)
                            .ok_or(BridgeError::BufferLimit {
                                requested: second,
                                limit: binding.limit,
                            })?;
                        let mut bytes = vec![0; len];
                        if len != 0 {
                            abi.validate_span(address, len)?;
                            if binding.mode.write() {
                                memory.check_write(address, len)?;
                            }
                            if binding.mode.read() {
                                memory.read(address, &mut bytes)?;
                            }
                        }
                        Input::Buffer {
                            bytes,
                            write_back: (binding.mode.write() && len != 0).then_some(address),
                        }
                    }
                    NativeFunction::HandleU64(_) => Input::Handle {
                        id: first,
                        value: second,
                        native: None,
                    },
                    _ => Input::Words(first, second),
                }
            }
            _ => unreachable!("registered ABI schemas"),
        };
        Ok(Self {
            abi,
            function,
            input,
            output,
            resume,
        })
    }

    pub(crate) fn handle(&self) -> Option<u64> {
        match self.input {
            Input::Handle { id, .. } => Some(id),
            _ => None,
        }
    }

    pub(crate) fn resolve_handle(&mut self, token: u64) {
        if let Input::Handle { native, .. } = &mut self.input {
            *native = Some(token);
        }
    }

    pub(crate) fn call_native(&mut self) -> Result<NativeResult, BridgeError> {
        let value = match (self.function, &mut self.input) {
            (NativeFunction::U64Binary(call), Input::Words(a, b)) => call(*a, *b),
            (NativeFunction::I64Binary(call), Input::Words(a, b)) => {
                call(*a as i64, *b as i64) as u64
            }
            (NativeFunction::U64F64(call), Input::Words(a, b)) => {
                call(*a, f64::from_bits(*b)).to_bits()
            }
            (NativeFunction::Pair(call), Input::Pair(pair)) => {
                return Ok(NativeResult::Pair(call(*pair)));
            }
            (NativeFunction::Buffer(call), Input::Buffer { bytes, .. }) => call.call(bytes),
            (NativeFunction::HandleU64(call), Input::Handle { id, value, native }) => {
                call(native.ok_or(BridgeError::InvalidHandle(*id))?, *value)
            }
            _ => unreachable!("prepared native schema"),
        };
        Ok(NativeResult::Word(value))
    }

    pub(crate) fn finish(
        self,
        cpu: &mut CpuState,
        memory: &mut dyn GuestMemory,
        result: NativeResult,
    ) -> Result<(), BridgeError> {
        if let Input::Buffer {
            bytes,
            write_back: Some(address),
        } = self.input
        {
            memory.write(address, &bytes)?;
        }
        match (self.output, result) {
            (Output::Word(location), NativeResult::Word(value)) => {
                self.abi.write_word(cpu, memory, location, value)?
            }
            (Output::Pair([first, second]), NativeResult::Pair(pair)) => {
                self.abi.write_word(cpu, memory, first, pair.first)?;
                self.abi.write_word(cpu, memory, second, pair.second)?;
            }
            (Output::Indirect { address, echo }, NativeResult::Pair(pair)) => {
                memory.write(address, &pair.guest_bytes())?;
                self.abi.write_word(cpu, memory, echo, address.0)?;
            }
            _ => unreachable!("prepared native result schema"),
        }
        self.resume.apply(cpu);
        Ok(())
    }
}

fn check_pair(
    abi: &AbiDescriptor,
    address: GuestAddress,
    alignment: u64,
) -> Result<(), BridgeError> {
    if !address.0.is_multiple_of(alignment) {
        return Err(BridgeError::MisalignedAggregate(address));
    }
    abi.validate_span(address, 16)?;
    Ok(())
}

#[derive(Debug)]
#[non_exhaustive]
pub enum BridgeError {
    Abi(AbiError),
    UnsupportedHost,
    UnknownModule(String),
    UnknownEntry(GuestAddress),
    DuplicateEntry(GuestAddress),
    DuplicateSymbol { module: String, symbol: String },
    EmptyBindingName,
    InvalidHandle(u64),
    WrongHandleType(u64),
    HandleIdsExhausted,
    BufferLimit { requested: u64, limit: usize },
    MisalignedAggregate(GuestAddress),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Abi(error) => error.fmt(f),
            Self::UnsupportedHost => f.write_str("the host native C ABI is unavailable"),
            Self::UnknownModule(module) => write!(f, "unknown bridge module {module}"),
            Self::UnknownEntry(address) => write!(f, "unknown bridge entry at {:#x}", address.0),
            Self::DuplicateEntry(address) => {
                write!(f, "duplicate bridge entry at {:#x}", address.0)
            }
            Self::DuplicateSymbol { module, symbol } => {
                write!(f, "duplicate bridge symbol {module}!{symbol}")
            }
            Self::EmptyBindingName => {
                f.write_str("bridge module and symbol names must be nonempty")
            }
            Self::InvalidHandle(handle) => write!(f, "invalid module-owned guest handle {handle}"),
            Self::WrongHandleType(handle) => {
                write!(f, "guest handle {handle} has the wrong resource type")
            }
            Self::HandleIdsExhausted => f.write_str("guest handle identifiers exhausted"),
            Self::BufferLimit { requested, limit } => {
                write!(f, "guest buffer length {requested} exceeds limit {limit}")
            }
            Self::MisalignedAggregate(address) => {
                write!(f, "misaligned guest aggregate at {:#x}", address.0)
            }
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

impl From<MemoryError> for BridgeError {
    fn from(error: MemoryError) -> Self {
        Self::Abi(error.into())
    }
}
