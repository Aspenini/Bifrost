//! Self-authored deterministic fixtures shared by Bifröst examples and tests.

pub mod arm64;
#[allow(unsafe_code)]
pub mod native;
pub mod x86;

use bifrost_core::{AbiId, GuestAddress, GuestIsa, GuestOs, GuestTarget};
use bifrost_ir::*;
use bifrost_memory::{AddressSpace, GuestMemory, MemoryError, Protection};

pub const FIXTURE_DATA: GuestAddress = GuestAddress(0x2000);
pub const FIXTURE_VALUE: u64 = 41;

/// Guest bytes for the input used by the scalar IR fixture.
pub fn scalar_memory() -> Result<AddressSpace, MemoryError> {
    let mut memory = AddressSpace::new(4096, 1)?;
    memory.map(FIXTURE_DATA, 4096, Protection::ReadWrite)?;
    memory.write(FIXTURE_DATA, &FIXTURE_VALUE.to_le_bytes())?;
    memory.protect(FIXTURE_DATA, 4096, Protection::Read)?;
    Ok(memory)
}

/// A hand-authored x86-64 IR fixture that loads a scalar and writes the increment to RAX.
/// Its PC range supplies guest location metadata for verification.
pub fn scalar_ir() -> IrFunction {
    let pc = GuestAddress(0x1000);
    let layout = MemoryLayout {
        ty: Type::I64,
        alignment: 8,
        endian: Endian::Little,
    };
    let instructions = [
        Op::Const {
            out: ValueId(0),
            ty: Type::GuestAddress,
            bits: u128::from(FIXTURE_DATA.0),
        },
        Op::Guard {
            out: ValueId(1),
            address: ValueId(0),
            access: MemoryAccess::Read,
            layout,
        },
        Op::Load {
            out: ValueId(2),
            guard: ValueId(1),
            layout,
        },
        Op::Const {
            out: ValueId(3),
            ty: Type::I64,
            bits: 1,
        },
        Op::Int {
            out: ValueId(4),
            op: IntOp::Add,
            lhs: ValueId(2),
            rhs: ValueId(3),
        },
        Op::WriteRegister {
            register: Register::X86(X86Reg::Gpr(0)),
            value: ValueId(4),
        },
        Op::Const {
            out: ValueId(5),
            ty: Type::GuestAddress,
            bits: 0x1001,
        },
    ];
    IrFunction {
        target: GuestTarget::new(
            GuestIsa::X86_64,
            GuestOs::None,
            AbiId::Bare,
            Default::default(),
        )
        .expect("valid bare x86-64 target"),
        entry: BlockId(0),
        blocks: vec![IrBlock {
            id: BlockId(0),
            span: CodeSpan { start: pc, len: 1 },
            parameters: vec![],
            instructions: instructions
                .into_iter()
                .map(|op| Instruction { pc, op })
                .collect(),
            exit_pc: pc,
            exit: Exit::NextPc(ValueId(5)),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_memory::GuestMemoryRead;

    #[test]
    fn verifies_and_prints_the_scalar_fixture() {
        let verified = scalar_ir().verify().unwrap();
        assert_eq!(verified.blocks().len(), 1);
        let printed = verified.to_string();
        assert!(printed.starts_with("bifrost-ir v1 X86_64 entry b0\n"));
        assert!(printed.contains("  0x1000  NextPc(ValueId(5))"));

        let memory = scalar_memory().unwrap();
        let mut bytes = [0; 8];
        memory.read(FIXTURE_DATA, &mut bytes).unwrap();
        assert_eq!(u64::from_le_bytes(bytes), FIXTURE_VALUE);
    }
}
