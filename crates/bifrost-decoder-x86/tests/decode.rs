use bifrost_core::GuestAddress as Addr;
use bifrost_decoder_x86::{
    Arithmetic, Condition, DecodeError, MemoryBase, MemoryOperand, Operand, Operation, decode,
    fetch,
};
use bifrost_memory::{AddressSpace, GuestMemory, MemoryError, Protection};

#[test]
fn retains_pc_encoding_and_normalized_operands() {
    let bytes = [0x4f, 0x8b, 0x54, 0x8c, 0xf8, 0x90];
    let decoded = decode(&bytes, Addr(0x1000)).unwrap();
    assert_eq!(decoded.pc(), Addr(0x1000));
    assert_eq!(decoded.next_pc(), Addr(0x1005));
    assert_eq!(decoded.bytes(), &bytes[..5]);
    assert_eq!(
        *decoded.operation(),
        Operation::Mov {
            dst: Operand::Register(10),
            src: Operand::Memory(MemoryOperand {
                base: MemoryBase::Register(12),
                index: Some((9, 4)),
                displacement: (-8_i64) as u64
            }),
        }
    );
    let rip = decode(&[0x48, 0x8b, 0x05, 0xfc, 0xff, 0xff, 0xff], Addr(0x1000)).unwrap();
    let Operation::Mov {
        src: Operand::Memory(address),
        ..
    } = rip.operation()
    else {
        panic!("expected RIP-relative MOV")
    };
    assert_eq!(address.base, MemoryBase::Rip);
    assert_eq!(address.displacement, (-4_i64) as u64);
    let add = decode(&[0x48, 0x83, 0xc0, 0xff], Addr(0)).unwrap();
    assert_eq!(
        *add.operation(),
        Operation::Arithmetic {
            op: Arithmetic::Add,
            dst: 0,
            src: Operand::Immediate(u64::MAX)
        }
    );
    let branch = decode(&[0x7c, 0xfe], Addr(0x1000)).unwrap();
    assert_eq!(
        *branch.operation(),
        Operation::Branch {
            condition: Some(Condition::Less),
            target: Addr(0x1000)
        }
    );
}

#[test]
fn distinguishes_unsupported_truncated_and_invalid_encodings() {
    let pc = Addr(0x10);
    for bytes in [
        &[0x0f, 0x05][..],
        &[0x67, 0x48, 0x8b, 0x00],
        &[0x48, 0x01, 0x18],
        &[0xf3, 0x90],
    ] {
        assert!(matches!(
            decode(bytes, pc).unwrap().operation(),
            Operation::Unsupported { .. }
        ));
    }
    assert_eq!(
        decode(&[0x48, 0xb8], pc),
        Err(DecodeError::Truncated { pc })
    );
    assert_eq!(decode(&[0x66; 15], pc), Err(DecodeError::Invalid { pc }));
    assert_eq!(
        *decode(&[0x0f, 0x0b], pc).unwrap().operation(),
        Operation::Ud2
    );
}

#[test]
fn fetches_only_required_executable_bytes() {
    let mut memory = AddressSpace::new(16, 2).unwrap();
    memory.map(Addr(0x10), 16, Protection::ReadWrite).unwrap();
    memory.write(Addr(0x1e), &[0x90, 0xeb]).unwrap();
    memory.protect(Addr(0x10), 16, Protection::Execute).unwrap();
    assert_eq!(
        *fetch(&memory, Addr(0x1e)).unwrap().operation(),
        Operation::Nop
    );
    assert_eq!(
        fetch(&memory, Addr(0x1f)),
        Err(DecodeError::Fetch {
            pc: Addr(0x1f),
            source: MemoryError::Unmapped(Addr(0x20))
        })
    );
    memory.map(Addr(0x20), 16, Protection::ReadWrite).unwrap();
    memory.write(Addr(0x20), &[0xfe]).unwrap();
    assert!(matches!(
        fetch(&memory, Addr(0x1f)),
        Err(DecodeError::Fetch {
            source: MemoryError::Denied { .. },
            ..
        })
    ));
    memory.protect(Addr(0x20), 16, Protection::Execute).unwrap();
    assert_eq!(fetch(&memory, Addr(0x1f)).unwrap().bytes(), &[0xeb, 0xfe]);
}
