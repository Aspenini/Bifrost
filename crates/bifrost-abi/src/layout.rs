use super::{AbiDescriptor, AbiError, GuestIsa, Location, PairPassing, Scalar};

/// The bridge's supported, C-compatible, two-word integer aggregate.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pair64 {
    pub first: u64,
    pub second: u64,
}

impl Pair64 {
    pub fn from_guest_bytes(bytes: [u8; 16]) -> Self {
        let (first, second) = bytes.split_at(8);
        Self {
            first: u64::from_le_bytes(first.try_into().unwrap()),
            second: u64::from_le_bytes(second.try_into().unwrap()),
        }
    }

    pub fn guest_bytes(self) -> [u8; 16] {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&self.first.to_le_bytes());
        bytes[8..].copy_from_slice(&self.second.to_le_bytes());
        bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Argument {
    Scalar(Scalar),
    Pair64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArgumentLocation {
    Word(Location),
    Pair([Location; 2]),
    IndirectPair(Location),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultLocation {
    Word(Location),
    Pair([Location; 2]),
    IndirectPair { pointer: Location, echo: Location },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallLayout {
    pub arguments: Vec<ArgumentLocation>,
    pub result: ResultLocation,
}

impl AbiDescriptor {
    pub fn layout(&self, arguments: &[Argument], result: Argument) -> Result<CallLayout, AbiError> {
        let mut cursor = self.cursor();
        let result = match result {
            Argument::Scalar(kind) => ResultLocation::Word(self.scalar_return(kind)),
            Argument::Pair64 if self.pair_passing == PairPassing::IndirectAligned16 => {
                ResultLocation::IndirectPair {
                    pointer: cursor.scalar(Scalar::U64)?,
                    echo: self.scalar_return(Scalar::U64),
                }
            }
            Argument::Pair64 => ResultLocation::Pair([
                Location::Integer(self.integer_returns[0]),
                Location::Integer(self.integer_returns[1]),
            ]),
        };
        let arguments = arguments
            .iter()
            .map(|argument| {
                Ok(match argument {
                    Argument::Scalar(kind) => ArgumentLocation::Word(cursor.scalar(*kind)?),
                    Argument::Pair64 if self.pair_passing == PairPassing::IndirectAligned16 => {
                        ArgumentLocation::IndirectPair(cursor.scalar(Scalar::U64)?)
                    }
                    Argument::Pair64 if cursor.integer + 2 <= self.integer_args.len() => {
                        ArgumentLocation::Pair([
                            cursor.scalar(Scalar::U64)?,
                            cursor.scalar(Scalar::U64)?,
                        ])
                    }
                    Argument::Pair64 => {
                        if self.isa == GuestIsa::AArch64 {
                            cursor.integer = self.integer_args.len();
                        }
                        ArgumentLocation::Pair([cursor.stack_slot()?, cursor.stack_slot()?])
                    }
                })
            })
            .collect::<Result<_, AbiError>>()?;
        Ok(CallLayout { arguments, result })
    }
}
